//! Game.memory：主进程受控数据树（docs/architecture/06）。
//!
//! - 根为映射；映射键只接受字符串；映射按插入序枚举。
//! - 值允许 null / 布尔 / 有限数字 / 字符串 / 列表 / 字符串键映射。
//! - 写入原生容器时由宿主深拷贝为线值，主进程再次校验；每次修改原子
//!   提交或整体拒绝并增加修订号。
//! - 句柄通过会话代次 + 容器节点 id 寻址；祖先被替换或删除后旧句柄失效
//!   （STALE_MEMORY_REFERENCE）。
//! - 初始化写临时分支，成功才整体提交。
//! - `robots` 为保留根键；`r.memory` 自动创建；`_move` 为绑定保留键。
//!
//! A0 最小集：映射读取 / 赋值 / 删除 / 成员检查 / 长度 / 有序键迭代，
//! 列表下标读写 / 长度 / 迭代 / 追加 / 删除指定下标，to_dict / to_list。
//!
//! 实现约定：节点存于 arena，id 即下标、永不复用。写入分两步：
//! 先旁路构建（`BuildCtx`，容器后序遍历，子节点 id = arena.len() + 构建序），
//! 校验限额通过后一次性落位并从根重算字节 / 节点数；失败则树原样不动。

use ztw_model::MemValue;

pub type NodeId = u64;

/// 每节点固定开销（与 `MemValue::approx_bytes` 同口径）。
const NODE_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Map,
    List,
}

/// 节点角色：保留键裁决（docs/architecture/06）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Root,
    /// 根下保留键 robots 对应的映射。
    Robots,
    /// robots/<id> 下的机器人记忆映射（_move 保留）。
    RobotMem,
    Plain,
}

#[derive(Debug, Clone)]
enum Slot {
    Val(MemValue), // 标量（构建时保证非容器）
    Node(NodeId),
}

#[derive(Debug, Clone)]
struct Node {
    alive: bool,
    kind: NodeKind,
    tag: Tag,
    /// 映射：有序 (键, 槽)；列表：有序槽（键恒为空串，位置即下标）。
    entries: Vec<(String, Slot)>,
}

#[derive(Debug, Clone)]
pub struct MemoryLimits {
    pub max_nodes: usize,
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_string: usize,
}

impl Default for MemoryLimits {
    fn default() -> Self {
        MemoryLimits {
            max_nodes: 10_000,
            max_bytes: 256 * 1024,
            max_depth: 16,
            max_string: 8 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryTree {
    arena: Vec<Node>,
    /// 从根可达的节点数与字节数（每次成功修改后重算）。
    node_count: usize,
    bytes: usize,
    revision: u64,
    /// 会话代次：初始化提交、环境重建、宿主重启时递增，旧句柄失效。
    generation: u64,
    limits: MemoryLimits,
}

/// 读取结果：标量值拷贝或容器句柄。
#[derive(Debug, Clone, PartialEq)]
pub enum ReadResult {
    Scalar(MemValue),
    Handle { node: NodeId, kind: NodeKind },
    Missing,
}

#[derive(Debug)]
pub struct MemOpError {
    pub code: &'static str,
    pub message: String,
}

impl MemOpError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        MemOpError {
            code,
            message: message.into(),
        }
    }
}

/// 旁路构建上下文：`base` 为本次构建节点在 arena 尾部的起始下标。
struct BuildCtx {
    base: NodeId,
    nodes: Vec<Node>,
    node_total: usize,
    bytes_total: usize,
}

impl MemoryTree {
    pub fn new(limits: MemoryLimits) -> MemoryTree {
        let mut t = MemoryTree {
            arena: Vec::new(),
            node_count: 0,
            bytes: 0,
            revision: 0,
            generation: 0,
            limits,
        };
        t.arena.push(Node {
            alive: true,
            kind: NodeKind::Map,
            tag: Tag::Root,
            entries: Vec::new(),
        });
        t.recompute();
        t
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    // -- 内部工具 -----------------------------------------------------------

    fn live(&self, id: NodeId) -> Result<&Node, MemOpError> {
        self.arena
            .get(id as usize)
            .filter(|n| n.alive)
            .ok_or_else(|| MemOpError::new("STALE_MEMORY_REFERENCE", "容器已失效"))
    }

    fn live_mut(&mut self, id: NodeId) -> Result<&mut Node, MemOpError> {
        self.arena
            .get_mut(id as usize)
            .filter(|n| n.alive)
            .ok_or_else(|| MemOpError::new("STALE_MEMORY_REFERENCE", "容器已失效"))
    }

    fn check_gen(&self, session_gen: u64) -> Result<(), MemOpError> {
        if session_gen != self.generation {
            return Err(MemOpError::new(
                "STALE_MEMORY_REFERENCE",
                format!("句柄代次 {session_gen} 与当前 {} 不符", self.generation),
            ));
        }
        Ok(())
    }

    fn check_map(&self, id: NodeId) -> Result<(), MemOpError> {
        if self.live(id)?.kind != NodeKind::Map {
            return Err(MemOpError::new("WRONG_NODE_KIND", "目标不是映射"));
        }
        Ok(())
    }

    fn check_list(&self, id: NodeId) -> Result<(), MemOpError> {
        if self.live(id)?.kind != NodeKind::List {
            return Err(MemOpError::new("WRONG_NODE_KIND", "目标不是列表"));
        }
        Ok(())
    }

    fn slot_bytes(&self, s: &Slot) -> usize {
        match s {
            Slot::Val(MemValue::Str(v)) => NODE_BYTES + v.len(),
            Slot::Val(_) => NODE_BYTES,
            Slot::Node(c) => self.subtree_bytes(*c),
        }
    }

    fn subtree_bytes(&self, id: NodeId) -> usize {
        let Some(n) = self.arena.get(id as usize) else {
            return 0;
        };
        NODE_BYTES
            + n.entries
                .iter()
                .map(|(k, s)| k.len() + self.slot_bytes(s))
                .sum::<usize>()
    }

    fn subtree_nodes(&self, id: NodeId) -> usize {
        let Some(n) = self.arena.get(id as usize) else {
            return 0;
        };
        // 与 build_inner 的 node_total 同口径：容器与标量叶都计 1，
        // 否则 max_nodes 检查会因度量不一致被实际超限。
        1 + n
            .entries
            .iter()
            .map(|(_, s)| match s {
                Slot::Node(c) => self.subtree_nodes(*c),
                Slot::Val(_) => 1,
            })
            .sum::<usize>()
    }

    fn recompute(&mut self) {
        self.node_count = self.subtree_nodes(0);
        self.bytes = self.subtree_bytes(0);
    }

    /// 从根到该节点的深度（根为 0）。
    fn depth_of(&self, target: NodeId) -> usize {
        fn walk(t: &MemoryTree, id: NodeId, d: usize, target: NodeId, out: &mut Option<usize>) {
            if out.is_some() {
                return;
            }
            if id == target {
                *out = Some(d);
                return;
            }
            if let Some(n) = t.arena.get(id as usize) {
                for (_, s) in &n.entries {
                    if let Slot::Node(c) = s {
                        walk(t, *c, d + 1, target, out);
                    }
                }
            }
        }
        let mut out = None;
        walk(self, 0, 0, target, &mut out);
        out.unwrap_or(0)
    }

    fn kill_subtree(&mut self, id: NodeId) {
        let children: Vec<NodeId> = match self.arena.get(id as usize) {
            Some(n) => n
                .entries
                .iter()
                .filter_map(|(_, s)| match s {
                    Slot::Node(c) => Some(*c),
                    _ => None,
                })
                .collect(),
            None => return,
        };
        if let Some(n) = self.arena.get_mut(id as usize) {
            n.alive = false;
        }
        for c in children {
            self.kill_subtree(c);
        }
    }

    /// 旁路构建：线值 → 槽 + 待追加节点。返回根槽。
    /// 深度不在此检查（限额校验在写入入口按 value.depth() 统一判定）。
    fn build(&self, v: &MemValue) -> Result<(Slot, BuildCtx), MemOpError> {
        let mut ctx = BuildCtx {
            base: self.arena.len() as NodeId,
            nodes: Vec::new(),
            node_total: 0,
            bytes_total: 0,
        };
        let slot = self.build_inner(v, &mut ctx)?;
        Ok((slot, ctx))
    }

    /// 容器后序遍历：子节点先入 ctx.nodes，父节点后入；id = base + 构建序。
    fn build_inner(&self, v: &MemValue, ctx: &mut BuildCtx) -> Result<Slot, MemOpError> {
        match v {
            MemValue::List(_) | MemValue::Map(_) => {
                let (pairs, kind): (Vec<(String, MemValue)>, NodeKind) = match v {
                    MemValue::Map(pairs) => (pairs.clone(), NodeKind::Map),
                    MemValue::List(items) => (
                        items.iter().map(|it| (String::new(), it.clone())).collect(),
                        NodeKind::List,
                    ),
                    _ => unreachable!(),
                };
                let mut entries = Vec::with_capacity(pairs.len());
                for (k, child) in &pairs {
                    if k.len() > self.limits.max_string {
                        return Err(MemOpError::new(
                            "MEMORY_LIMIT",
                            format!("键长度 {} 超上限 {}", k.len(), self.limits.max_string),
                        ));
                    }
                    let s = self.build_inner(child, ctx)?;
                    entries.push((k.clone(), s));
                }
                let node = Node {
                    alive: true,
                    kind,
                    tag: Tag::Plain,
                    entries,
                };
                let id = ctx.base + ctx.nodes.len() as NodeId;
                ctx.nodes.push(node);
                ctx.node_total += 1;
                ctx.bytes_total += NODE_BYTES + pairs.iter().map(|(k, _)| k.len()).sum::<usize>();
                Ok(Slot::Node(id))
            }
            scalar => {
                scalar
                    .validate(self.limits.max_string)
                    .map_err(|m| MemOpError::new("INVALID_VALUE", m))?;
                ctx.node_total += 1;
                ctx.bytes_total += match scalar {
                    MemValue::Str(s) => NODE_BYTES + s.len(),
                    _ => NODE_BYTES,
                };
                Ok(Slot::Val(scalar.clone()))
            }
        }
    }

    /// 写入统一入口：限额校验 → 落位 → 重算 → 修订号 +1。
    /// `key` 为映射键；列表路径（append）不检查保留键。
    /// 任何校验失败时树原样不动（docs/architecture/06 原子性）。
    fn write_slot(
        &mut self,
        container: NodeId,
        key: Option<&str>,
        value: &MemValue,
    ) -> Result<(), MemOpError> {
        let depth_here = self.depth_of(container);
        if let Some(k) = key
            && k.len() > self.limits.max_string
        {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("键长度 {} 超上限 {}", k.len(), self.limits.max_string),
            ));
        }
        let (slot, ctx) = self.build(value)?;
        self.verify_built_ids(&ctx);
        // 被替换旧槽的字节 / 节点（映射按键定位；列表 append 无旧槽）。
        let old = match key {
            Some(k) => self
                .live(container)?
                .entries
                .iter()
                .find(|(ek, _)| ek == k)
                .map(|(_, s)| s.clone()),
            None => None,
        };
        let key_len = key.map(|k| k.len()).unwrap_or(0);
        let old_bytes = old
            .as_ref()
            .map(|s| self.slot_bytes(s) + key_len)
            .unwrap_or(0);
        let old_nodes = old
            .as_ref()
            .map(|s| match s {
                Slot::Node(c) => self.subtree_nodes(*c),
                Slot::Val(_) => 1,
            })
            .unwrap_or(0);
        // 新侧必须计入被写键本身的字节（旧侧已含于 old_bytes）。
        if self.node_count + ctx.node_total - old_nodes > self.limits.max_nodes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("节点数超上限 {}", self.limits.max_nodes),
            ));
        }
        if self.bytes + ctx.bytes_total + key_len - old_bytes > self.limits.max_bytes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("字节数超上限 {}", self.limits.max_bytes),
            ));
        }
        if depth_here + value.depth() > self.limits.max_depth {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("深度超上限 {}", self.limits.max_depth),
            ));
        }
        // 校验全部通过，落位。
        if let Some(Slot::Node(c)) = &old {
            self.kill_subtree(*c);
        }
        self.arena.extend(ctx.nodes);
        let n = self.live_mut(container)?;
        match key {
            Some(k) => match n.entries.iter_mut().find(|(ek, _)| ek == k) {
                Some(e) => e.1 = slot,
                None => n.entries.push((k.to_string(), slot)),
            },
            None => n.entries.push((String::new(), slot)),
        }
        self.recompute();
        self.revision += 1;
        Ok(())
    }

    /// 校验旁路构建的节点 id 约定：id ∈ [base, base+len) 且子先于父
    /// （后序构建），落到 arena 后 id 即最终下标。违反即内部 bug。
    fn verify_built_ids(&self, ctx: &BuildCtx) {
        let len = ctx.nodes.len() as NodeId;
        debug_assert!(
            ctx.nodes
                .iter()
                .enumerate()
                .all(|(i, n)| n.entries.iter().all(|(_, s)| match s {
                    // 子先于父（后序构建）：子 id 的构建序 < 父的构建序 i。
                    Slot::Node(c) => {
                        *c >= ctx.base && *c < ctx.base + len && ((*c - ctx.base) as usize) < i
                    }
                    _ => true,
                })),
            "built 节点 id 违反后序约定"
        );
    }

    // -- 对外操作（每次修改原子提交或整体拒绝） ------------------------------

    pub fn map_get(
        &mut self,
        session_gen: u64,
        node: NodeId,
        key: &str,
    ) -> Result<ReadResult, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_map(node)?;
        Ok(self.read_slot(
            self.live(node)?
                .entries
                .iter()
                .find(|(k, _)| k == key)
                .map(|p| &p.1),
        ))
    }

    fn read_slot(&self, slot: Option<&Slot>) -> ReadResult {
        match slot {
            Some(Slot::Val(v)) => ReadResult::Scalar(v.clone()),
            Some(Slot::Node(c)) => match self.arena.get(*c as usize).filter(|n| n.alive) {
                Some(n) => ReadResult::Handle {
                    node: *c,
                    kind: n.kind,
                },
                None => ReadResult::Missing, // id 不变量破坏时的防御：不 panic
            },
            None => ReadResult::Missing,
        }
    }

    pub fn map_set(
        &mut self,
        session_gen: u64,
        node: NodeId,
        key: &str,
        value: &MemValue,
    ) -> Result<(), MemOpError> {
        self.check_gen(session_gen)?;
        self.check_map(node)?;
        self.reserved_key_check(node, key)?;
        self.write_slot(node, Some(key), value)
    }

    pub fn map_delete(
        &mut self,
        session_gen: u64,
        node: NodeId,
        key: &str,
    ) -> Result<bool, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_map(node)?;
        self.reserved_key_check(node, key)?;
        let pos = self.live(node)?.entries.iter().position(|(k, _)| k == key);
        let Some(pos) = pos else {
            return Ok(false);
        };
        let (_, slot) = self.live_mut(node)?.entries.remove(pos);
        if let Slot::Node(c) = slot {
            self.kill_subtree(c);
        }
        self.recompute();
        self.revision += 1;
        Ok(true)
    }

    fn reserved_key_check(&self, node: NodeId, key: &str) -> Result<(), MemOpError> {
        match (self.live(node)?.tag, key) {
            (Tag::Root, "robots") => Err(MemOpError::new(
                "RESERVED_KEY",
                "robots 是保留根键，不能整体替换或删除",
            )),
            // robots/<id> 条目由 r.memory 管理：阻断直接写入/删除，防止
            // 非 RobotMem 槽位混入（键唯一性与 _move 保留的前提）。
            // 读写 robots/<id> 内部请经 r.memory。
            (Tag::Robots, _) => Err(MemOpError::new(
                "RESERVED_KEY",
                "robots 下的机器人条目由 r.memory 管理，请使用 r.memory 读写",
            )),
            (Tag::RobotMem, "_move") => Err(MemOpError::new(
                "RESERVED_KEY",
                "_move 由 move_to 绑定管理，玩家不得写入",
            )),
            _ => Ok(()),
        }
    }

    pub fn map_has(
        &mut self,
        session_gen: u64,
        node: NodeId,
        key: &str,
    ) -> Result<bool, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_map(node)?;
        Ok(self.live(node)?.entries.iter().any(|(k, _)| k == key))
    }

    pub fn map_size(&mut self, session_gen: u64, node: NodeId) -> Result<usize, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_map(node)?;
        Ok(self.live(node)?.entries.len())
    }

    /// 有序键快照（迭代开始时固定）。
    pub fn map_keys(&mut self, session_gen: u64, node: NodeId) -> Result<Vec<String>, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_map(node)?;
        Ok(self
            .live(node)?
            .entries
            .iter()
            .map(|(k, _)| k.clone())
            .collect())
    }

    pub fn list_get(
        &mut self,
        session_gen: u64,
        node: NodeId,
        index: usize,
    ) -> Result<ReadResult, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_list(node)?;
        Ok(self.read_slot(self.live(node)?.entries.get(index).map(|p| &p.1)))
    }

    pub fn list_set(
        &mut self,
        session_gen: u64,
        node: NodeId,
        index: usize,
        value: &MemValue,
    ) -> Result<(), MemOpError> {
        self.check_gen(session_gen)?;
        self.check_list(node)?;
        if index >= self.live(node)?.entries.len() {
            return Err(MemOpError::new(
                "INDEX_OUT_OF_RANGE",
                format!("下标 {index} 超出列表长度"),
            ));
        }
        // 原子替换：先在旁路完成构建与全部限额校验（扣除被替换旧槽），
        // 全部通过后才移除旧槽、落位新值——拒绝路径树原样不动。
        let depth_here = self.depth_of(node);
        let (slot, ctx) = self.build(value)?;
        self.verify_built_ids(&ctx);
        let old = self.live(node)?.entries[index].1.clone();
        let old_bytes = self.slot_bytes(&old);
        let old_nodes = match &old {
            Slot::Node(c) => self.subtree_nodes(*c),
            Slot::Val(_) => 1,
        };
        if self.node_count + ctx.node_total - old_nodes > self.limits.max_nodes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("节点数超上限 {}", self.limits.max_nodes),
            ));
        }
        if self.bytes + ctx.bytes_total - old_bytes > self.limits.max_bytes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("字节数超上限 {}", self.limits.max_bytes),
            ));
        }
        if depth_here + value.depth() > self.limits.max_depth {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("深度超上限 {}", self.limits.max_depth),
            ));
        }
        if let Slot::Node(c) = old {
            self.kill_subtree(c);
        }
        self.arena.extend(ctx.nodes);
        let n = self.live_mut(node)?;
        n.entries[index].1 = slot;
        self.recompute();
        self.revision += 1;
        Ok(())
    }

    pub fn list_append(
        &mut self,
        session_gen: u64,
        node: NodeId,
        value: &MemValue,
    ) -> Result<(), MemOpError> {
        self.check_gen(session_gen)?;
        self.check_list(node)?;
        self.write_slot(node, None, value)
    }

    pub fn list_remove(
        &mut self,
        session_gen: u64,
        node: NodeId,
        index: usize,
    ) -> Result<(), MemOpError> {
        self.check_gen(session_gen)?;
        self.check_list(node)?;
        let len = self.live(node)?.entries.len();
        if index >= len {
            return Err(MemOpError::new(
                "INDEX_OUT_OF_RANGE",
                format!("下标 {index} 超出列表长度 {len}"),
            ));
        }
        let (_, slot) = self.live_mut(node)?.entries.remove(index);
        if let Slot::Node(c) = slot {
            self.kill_subtree(c);
        }
        self.recompute();
        self.revision += 1;
        Ok(())
    }

    pub fn list_size(&mut self, session_gen: u64, node: NodeId) -> Result<usize, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_list(node)?;
        Ok(self.live(node)?.entries.len())
    }

    /// 列表元素快照（迭代开始时固定）。
    pub fn list_entries(
        &mut self,
        session_gen: u64,
        node: NodeId,
    ) -> Result<Vec<ReadResult>, MemOpError> {
        self.check_gen(session_gen)?;
        self.check_list(node)?;
        Ok(self
            .live(node)?
            .entries
            .iter()
            .map(|(_, s)| self.read_slot(Some(s)))
            .collect())
    }

    /// `r.memory`：根下 robots 映射中该机器人的子映射，不存在或槽位
    /// 非法（历史数据被写坏）时原位替换为全新 RobotMem 节点——
    /// 绝不追加第二个同名键（保持映射键唯一不变量）。
    pub fn robot_memory(
        &mut self,
        session_gen: u64,
        robot_id: ztw_model::Id,
    ) -> Result<NodeId, MemOpError> {
        self.check_gen(session_gen)?;
        let robots_node = self.ensure_robots_map()?;
        let key = robot_id.to_string();
        let existing = self
            .live(robots_node)?
            .entries
            .iter()
            .position(|(k, _)| *k == key);
        if let Some(pos) = existing {
            if let Slot::Node(c) = self.live(robots_node)?.entries[pos].1
                && self.arena[c as usize].tag == Tag::RobotMem
            {
                return Ok(c);
            }
            // 槽位不是 RobotMem 映射：原位替换（删除旧子树）。
            let id = self.arena.len() as NodeId;
            let old = self.live_mut(robots_node)?.entries[pos].1.clone();
            self.arena.push(Node {
                alive: true,
                kind: NodeKind::Map,
                tag: Tag::RobotMem,
                entries: Vec::new(),
            });
            if let Slot::Node(c) = old {
                self.kill_subtree(c);
            }
            self.live_mut(robots_node)?.entries[pos].1 = Slot::Node(id);
            self.recompute();
            self.revision += 1;
            return Ok(id);
        }
        if self.node_count + 1 > self.limits.max_nodes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("节点数超上限 {}", self.limits.max_nodes),
            ));
        }
        let id = self.arena.len() as NodeId;
        self.arena.push(Node {
            alive: true,
            kind: NodeKind::Map,
            tag: Tag::RobotMem,
            entries: Vec::new(),
        });
        self.live_mut(robots_node)?
            .entries
            .push((key, Slot::Node(id)));
        self.recompute();
        self.revision += 1;
        Ok(id)
    }

    /// 服务端 move_to 缓存读取（ops.rs 复合逻辑专用）。`robots/<id>` 不存在
    /// 时会经 robot_memory 惰性创建空槽（无内容副作用）；`_move` 缺失或
    /// 形状异常返回原值，由调用方按缓存 miss 整体重寻——缓存只影响效率，
    /// 不构成权威状态。
    pub fn server_move_cache_get(&mut self, robot_id: ztw_model::Id) -> Option<MemValue> {
        let node = self.robot_memory(self.generation, robot_id).ok()?;
        let slot = self
            .live(node)
            .ok()?
            .entries
            .iter()
            .find(|(k, _)| k == "_move")
            .map(|p| p.1.clone())?;
        match slot {
            Slot::Node(c) => self.value_of(c).ok(),
            Slot::Val(v) => Some(v),
        }
    }

    /// 服务端 move_to 缓存写入：绕过 `reserved_key_check`（它挡的是玩家
    /// 写入，服务端是 `_move` 的唯一管理者），但 write_slot 的限额 / 深度 /
    /// 原子落位与修订号递增照常。限额拒绝由调用方降级为「不缓存」。
    pub fn server_move_cache_set(
        &mut self,
        robot_id: ztw_model::Id,
        value: &MemValue,
    ) -> Result<(), MemOpError> {
        let node = self.robot_memory(self.generation, robot_id)?;
        self.write_slot(node, Some("_move"), value)
    }

    fn ensure_robots_map(&mut self) -> Result<NodeId, MemOpError> {
        if let Some(Slot::Node(c)) = self.self_node_slot("robots") {
            return Ok(c);
        }
        if self.node_count + 1 > self.limits.max_nodes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("节点数超上限 {}", self.limits.max_nodes),
            ));
        }
        let id = self.arena.len() as NodeId;
        self.arena.push(Node {
            alive: true,
            kind: NodeKind::Map,
            tag: Tag::Robots,
            entries: Vec::new(),
        });
        self.live_mut(0)
            .expect("根节点常在")
            .entries
            .push(("robots".into(), Slot::Node(id)));
        self.recompute();
        self.revision += 1;
        Ok(id)
    }

    fn self_node_slot(&self, key: &str) -> Option<Slot> {
        self.arena
            .first()?
            .entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, s)| s.clone())
    }

    /// to_dict / to_list：深拷贝为线值。
    pub fn to_value(&mut self, session_gen: u64, node: NodeId) -> Result<MemValue, MemOpError> {
        self.check_gen(session_gen)?;
        self.value_of(node)
    }

    fn value_of(&self, node: NodeId) -> Result<MemValue, MemOpError> {
        let n = self.live(node)?;
        Ok(match n.kind {
            NodeKind::Map => MemValue::Map(
                n.entries
                    .iter()
                    .map(|(k, s)| Ok((k.clone(), self.slot_value(s)?)))
                    .collect::<Result<Vec<_>, MemOpError>>()?,
            ),
            NodeKind::List => MemValue::List(
                n.entries
                    .iter()
                    .map(|(_, s)| self.slot_value(s))
                    .collect::<Result<Vec<_>, MemOpError>>()?,
            ),
        })
    }

    fn slot_value(&self, s: &Slot) -> Result<MemValue, MemOpError> {
        match s {
            Slot::Val(v) => Ok(v.clone()),
            Slot::Node(c) => self.value_of(*c),
        }
    }

    // -- 初始化临时分支（docs/architecture/06 初始化与热重载） ----------------

    /// fork 出临时分支：初始化期间的 memory 写入落在这里。
    pub fn fork(&self) -> MemoryTree {
        self.clone()
    }

    /// 初始化成功后提交分支（代次 +1，旧句柄失效）。
    pub fn commit_branch(&mut self, branch: MemoryTree) {
        *self = branch;
        self.generation += 1;
        self.revision += 1;
    }

    /// 环境重建 / 宿主重启后重建句柄：代次 +1，内容不变。
    pub fn bump_generation(&mut self) {
        self.generation += 1;
    }

    /// 根节点整树快照（测试与持久化视图）。
    pub fn snapshot(&mut self) -> MemValue {
        self.value_of(0).unwrap_or(MemValue::Map(Vec::new()))
    }

    /// 读档重建（docs/architecture/06：节点 id 不属于存档公开语义，
    /// arena 重建并恢复根 / robots / robots\<id\> 的角色标签）。限额与
    /// 写入路径同标准整体复核（"不得只在存档时检测"）；任何拒绝按坏档
    /// 处理、不产出半棵树。修订号按存档恢复；generation 归零（读档后
    /// load_program 的初始化提交自会递增）。
    pub fn from_snapshot(
        root: MemValue,
        revision: u64,
        limits: MemoryLimits,
    ) -> Result<MemoryTree, MemOpError> {
        let MemValue::Map(pairs) = root else {
            return Err(MemOpError::new(
                "INVALID_VALUE",
                "memory 树根必须是映射（存档损坏）",
            ));
        };
        let mut ctx = BuildCtx {
            // 根占用 arena 下标 0，子树从 1 起（recompute / live 均以 0 为
            // 根寻址）；子先于父的后序构建照旧，最后把根插回下标 0。
            base: 1,
            nodes: Vec::new(),
            node_total: 0,
            bytes_total: 0,
        };
        let mut root_entries: Vec<(String, Slot)> = Vec::with_capacity(pairs.len());
        let mut root_seen = std::collections::BTreeSet::new();
        for (k, v) in &pairs {
            if !root_seen.insert(k.as_str()) {
                return Err(MemOpError::new(
                    "INVALID_VALUE",
                    format!("根下重复键「{k}」（存档损坏）"),
                ));
            }
            if k.len() > limits.max_string {
                return Err(MemOpError::new(
                    "MEMORY_LIMIT",
                    format!("键长度 {} 超上限 {}", k.len(), limits.max_string),
                ));
            }
            if v.depth() > limits.max_depth {
                return Err(MemOpError::new(
                    "MEMORY_LIMIT",
                    format!("深度超上限 {}", limits.max_depth),
                ));
            }
            // 保留键 robots：角色子树（其下条目必须是映射——线上树只能经
            // r.memory 产生，标量即坏档；键须为机器人 id 的规范十进制串，
            // "007" 这类非规范形会与 robot_memory 按 "7" 寻址产生第二个
            // 同语义条目）。
            let slot = if k == "robots" {
                let MemValue::Map(robot_pairs) = v else {
                    return Err(MemOpError::new(
                        "INVALID_VALUE",
                        "robots 保留键必须是映射（存档损坏）",
                    ));
                };
                let mut entries = Vec::with_capacity(robot_pairs.len());
                let mut robots_seen = std::collections::BTreeSet::new();
                for (rk, rv) in robot_pairs {
                    if !rk
                        .parse::<ztw_model::Id>()
                        .is_ok_and(|id| id.to_string() == *rk)
                    {
                        return Err(MemOpError::new(
                            "INVALID_VALUE",
                            format!("robots 条目「{rk}」不是规范机器人 id（存档损坏）"),
                        ));
                    }
                    if !robots_seen.insert(rk.as_str()) {
                        return Err(MemOpError::new(
                            "INVALID_VALUE",
                            format!("robots 下重复键「{rk}」（存档损坏）"),
                        ));
                    }
                    let MemValue::Map(mem_pairs) = rv else {
                        return Err(MemOpError::new(
                            "INVALID_VALUE",
                            format!("robots/{rk} 必须是映射（线上树只能经 r.memory 产生）"),
                        ));
                    };
                    // `_move`（move_to 路径缓存）自 M4 起由服务端写入线上树，
                    // 存档中合法——但恒为标量（字符串）；容器形属手改档，
                    // 恢复会让玩家经 map_get 拿到容器句柄、随后被服务端
                    // 标量写杀死（破坏「服务端无结构性写」不变量），按坏档拒。
                    if let Some((_, mv)) = mem_pairs.iter().find(|(mk, _)| mk == "_move")
                        && !matches!(
                            mv,
                            MemValue::Null
                                | MemValue::Bool(_)
                                | MemValue::Num(_)
                                | MemValue::Str(_)
                        )
                    {
                        return Err(MemOpError::new(
                            "INVALID_VALUE",
                            format!("robots/{rk} 的 _move 必须是标量（存档损坏）"),
                        ));
                    }
                    let s = restore_build(&limits, &mut ctx, rv, Tag::RobotMem)?;
                    entries.push((rk.clone(), s));
                }
                let keys_len = robot_pairs.iter().map(|(k, _)| k.len()).sum::<usize>();
                finish_container(
                    &mut ctx,
                    Node {
                        alive: true,
                        kind: NodeKind::Map,
                        tag: Tag::Robots,
                        entries,
                    },
                    keys_len,
                )
            } else {
                restore_build(&limits, &mut ctx, v, Tag::Plain)?
            };
            root_entries.push((k.clone(), slot));
        }
        // 根落位下标 0；根键字节与 build_inner 的容器口径一致计入预算。
        let root_keys_len: usize = pairs.iter().map(|(k, _)| k.len()).sum();
        let node_total_before_root = ctx.node_total;
        let bytes_before_root = ctx.bytes_total;
        ctx.nodes.insert(
            0,
            Node {
                alive: true,
                kind: NodeKind::Map,
                tag: Tag::Root,
                entries: root_entries,
            },
        );
        ctx.node_total = node_total_before_root + 1;
        ctx.bytes_total = bytes_before_root + NODE_BYTES + root_keys_len;
        if ctx.node_total > limits.max_nodes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("节点数超上限 {}", limits.max_nodes),
            ));
        }
        if ctx.bytes_total > limits.max_bytes {
            return Err(MemOpError::new(
                "MEMORY_LIMIT",
                format!("字节数超上限 {}", limits.max_bytes),
            ));
        }
        let mut t = MemoryTree {
            arena: ctx.nodes,
            node_count: 0,
            bytes: 0,
            revision,
            generation: 0,
            limits,
        };
        t.recompute();
        debug_assert_eq!(
            (t.node_count, t.bytes),
            (ctx.node_total, ctx.bytes_total),
            "读档重建的限额预算须与重算口径一致"
        );
        Ok(t)
    }
}

/// 容器节点的落位与计账（与 build_inner 同口径：节点 + 键长；后序，
/// 子先于父）。
fn finish_container(ctx: &mut BuildCtx, node: Node, keys_len: usize) -> Slot {
    let id = ctx.base + ctx.nodes.len() as NodeId;
    ctx.nodes.push(node);
    ctx.node_total += 1;
    ctx.bytes_total += NODE_BYTES + keys_len;
    Slot::Node(id)
}

/// 读档旁路构建：与 build_inner 同口径的计账与校验，但节点标签由调用
/// 方指定（RobotMem / Plain）；容器一律后序（子先于父）。
fn restore_build(
    limits: &MemoryLimits,
    ctx: &mut BuildCtx,
    v: &MemValue,
    tag: Tag,
) -> Result<Slot, MemOpError> {
    match v {
        MemValue::List(items) => {
            let mut entries = Vec::with_capacity(items.len());
            for it in items {
                let s = restore_build(limits, ctx, it, Tag::Plain)?;
                entries.push((String::new(), s));
            }
            Ok(finish_container(
                ctx,
                Node {
                    alive: true,
                    kind: NodeKind::List,
                    tag,
                    entries,
                },
                0,
            ))
        }
        MemValue::Map(pairs) => {
            let mut entries = Vec::with_capacity(pairs.len());
            let mut seen = std::collections::BTreeSet::new();
            for (k, child) in pairs {
                if k.len() > limits.max_string {
                    return Err(MemOpError::new(
                        "MEMORY_LIMIT",
                        format!("键长度 {} 超上限 {}", k.len(), limits.max_string),
                    ));
                }
                if !seen.insert(k.as_str()) {
                    return Err(MemOpError::new(
                        "INVALID_VALUE",
                        format!("映射下重复键「{k}」（存档损坏）"),
                    ));
                }
                let s = restore_build(limits, ctx, child, Tag::Plain)?;
                entries.push((k.clone(), s));
            }
            let keys_len = pairs.iter().map(|(k, _)| k.len()).sum::<usize>();
            Ok(finish_container(
                ctx,
                Node {
                    alive: true,
                    kind: NodeKind::Map,
                    tag,
                    entries,
                },
                keys_len,
            ))
        }
        scalar => {
            scalar
                .validate(limits.max_string)
                .map_err(|m| MemOpError::new("INVALID_VALUE", m))?;
            ctx.node_total += 1;
            ctx.bytes_total += match scalar {
                MemValue::Str(s) => NODE_BYTES + s.len(),
                _ => NODE_BYTES,
            };
            Ok(Slot::Val(scalar.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> MemoryTree {
        MemoryTree::new(MemoryLimits::default())
    }

    fn num(v: f64) -> MemValue {
        MemValue::Num(v)
    }

    fn handle_of(t: &mut MemoryTree, key: &str) -> (NodeId, NodeKind) {
        match t.map_get(0, 0, key).unwrap() {
            ReadResult::Handle { node, kind } => (node, kind),
            other => panic!("期望句柄，得到 {other:?}"),
        }
    }

    #[test]
    fn map_ops_in_order() {
        let mut t = tree();
        t.map_set(0, 0, "b", &num(1.0)).unwrap();
        t.map_set(0, 0, "a", &num(2.0)).unwrap();
        t.map_set(0, 0, "b", &num(3.0)).unwrap(); // 覆写保序
        assert_eq!(
            t.map_keys(0, 0).unwrap(),
            vec!["b".to_string(), "a".to_string()]
        );
        assert_eq!(t.map_get(0, 0, "b").unwrap(), ReadResult::Scalar(num(3.0)));
        assert_eq!(t.map_size(0, 0).unwrap(), 2);
        assert!(t.map_has(0, 0, "a").unwrap());
        assert!(t.map_delete(0, 0, "a").unwrap());
        t.map_set(0, 0, "a", &num(9.0)).unwrap(); // 删除后重插置于末尾
        assert_eq!(
            t.map_keys(0, 0).unwrap(),
            vec!["b".to_string(), "a".to_string()]
        );
        assert!(!t.map_delete(0, 0, "zz").unwrap());
    }

    #[test]
    fn container_write_and_handle() {
        let mut t = tree();
        let list = MemValue::List(vec![num(1.0), num(2.0)]);
        t.map_set(0, 0, "items", &list).unwrap();
        let (node, kind) = handle_of(&mut t, "items");
        assert_eq!(kind, NodeKind::List);
        assert_eq!(t.list_size(0, node).unwrap(), 2);
        t.list_append(0, node, &num(3.0)).unwrap();
        assert_eq!(t.list_size(0, node).unwrap(), 3);
        t.list_set(0, node, 0, &num(10.0)).unwrap();
        assert_eq!(
            t.list_get(0, node, 0).unwrap(),
            ReadResult::Scalar(num(10.0))
        );
        t.list_remove(0, node, 2).unwrap();
        assert_eq!(t.list_size(0, node).unwrap(), 2);
        // 嵌套容器句柄。
        t.list_append(0, node, &MemValue::Map(vec![("k".into(), num(5.0))]))
            .unwrap();
        match t.list_get(0, node, 2).unwrap() {
            ReadResult::Handle { node: sub, kind } => {
                assert_eq!(kind, NodeKind::Map);
                t.map_set(0, sub, "k2", &MemValue::Str("v".into())).unwrap();
            }
            other => panic!("期望句柄，得到 {other:?}"),
        }
        assert_eq!(
            t.to_value(0, 0).unwrap(),
            MemValue::Map(vec![(
                "items".into(),
                MemValue::List(vec![
                    num(10.0),
                    num(2.0),
                    MemValue::Map(vec![
                        ("k".into(), num(5.0)),
                        ("k2".into(), MemValue::Str("v".into()))
                    ])
                ])
            )])
        );
    }

    #[test]
    fn reject_keeps_tree_unchanged() {
        let mut t = tree();
        t.map_set(0, 0, "x", &num(1.0)).unwrap();
        let before = t.snapshot();
        let rev = t.revision();
        assert!(
            t.map_set(0, 0, "y", &MemValue::Str("x".repeat(9 * 1024)))
                .is_err()
        );
        assert!(t.map_set(0, 0, "y", &MemValue::Num(f64::NAN)).is_err());
        assert!(t.map_set(0, 0, "y", &MemValue::Num(1e16)).is_err());
        assert_eq!(t.snapshot(), before);
        assert_eq!(t.revision(), rev);
    }

    #[test]
    fn limits_reject_atomically() {
        let mut t = MemoryTree::new(MemoryLimits {
            max_nodes: 4,
            max_bytes: 256,
            max_depth: 3,
            max_string: 32,
        });
        t.map_set(0, 0, "a", &num(1.0)).unwrap();
        let before = t.snapshot();
        let deep = MemValue::Map(vec![(
            "l1".into(),
            MemValue::List(vec![MemValue::Map(vec![(
                "l2".into(),
                MemValue::List(vec![num(1.0)]),
            )])]),
        )]);
        assert!(t.map_set(0, 0, "deep", &deep).is_err());
        let many = MemValue::List((0..16).map(|i| num(i as f64)).collect());
        assert!(t.map_set(0, 0, "many", &many).is_err());
        let big = MemValue::Str("z".repeat(64));
        assert!(t.map_set(0, 0, "big", &big).is_err());
        assert_eq!(t.snapshot(), before);
    }

    #[test]
    fn stale_handle_after_replace() {
        let mut t = tree();
        t.map_set(0, 0, "m", &MemValue::Map(vec![("k".into(), num(1.0))]))
            .unwrap();
        let (node, _) = handle_of(&mut t, "m");
        t.map_set(0, 0, "m", &num(0.0)).unwrap();
        assert_eq!(
            t.map_get(0, node, "k").unwrap_err().code,
            "STALE_MEMORY_REFERENCE"
        );
        // 新数据不受旧句柄影响。
        t.map_set(0, 0, "m", &MemValue::Map(vec![("k".into(), num(7.0))]))
            .unwrap();
        let (node2, _) = handle_of(&mut t, "m");
        assert_ne!(node, node2);
        assert_eq!(
            t.map_get(0, node2, "k").unwrap(),
            ReadResult::Scalar(num(7.0))
        );
    }

    #[test]
    fn reserved_keys() {
        let mut t = tree();
        assert_eq!(
            t.map_set(0, 0, "robots", &num(1.0)).unwrap_err().code,
            "RESERVED_KEY"
        );
        assert_eq!(
            t.map_delete(0, 0, "robots").unwrap_err().code,
            "RESERVED_KEY"
        );
        let rm = t.robot_memory(0, 7).unwrap();
        assert_eq!(
            t.map_set(0, rm, "_move", &num(1.0)).unwrap_err().code,
            "RESERVED_KEY"
        );
        // r.memory 与逐层访问是同一节点。
        let (robots, _) = handle_of(&mut t, "robots");
        let via_path = match t.map_get(0, robots, "7").unwrap() {
            ReadResult::Handle { node, .. } => node,
            other => panic!("期望句柄，得到 {other:?}"),
        };
        assert_eq!(rm, via_path);
        t.map_set(0, rm, "note", &MemValue::Str("hi".into()))
            .unwrap();
        assert_eq!(
            t.map_get(0, via_path, "note").unwrap(),
            ReadResult::Scalar(MemValue::Str("hi".into()))
        );
    }

    #[test]
    fn generation_guard() {
        let mut t = tree();
        assert!(t.map_get(99, 0, "x").is_err());
        t.bump_generation();
        assert_eq!(
            t.map_get(0, 0, "x").unwrap_err().code,
            "STALE_MEMORY_REFERENCE"
        );
        assert!(t.map_get(1, 0, "x").is_ok());
    }

    #[test]
    fn fork_commit_semantics() {
        let mut t = tree();
        t.map_set(0, 0, "committed", &num(1.0)).unwrap();
        let mut branch = t.fork();
        branch.map_set(0, 0, "temp", &num(2.0)).unwrap();
        assert_eq!(t.map_get(0, 0, "temp").unwrap(), ReadResult::Missing);
        let stale_gen = t.generation();
        t.commit_branch(branch);
        let session_gen = t.generation();
        assert_eq!(session_gen, stale_gen + 1);
        // 旧代次句柄失效（提交即重建句柄）。
        assert_eq!(
            t.map_get(stale_gen, 0, "temp").unwrap_err().code,
            "STALE_MEMORY_REFERENCE"
        );
        assert_eq!(
            t.map_get(session_gen, 0, "temp").unwrap(),
            ReadResult::Scalar(num(2.0))
        );
        assert_eq!(
            t.map_get(session_gen, 0, "committed").unwrap(),
            ReadResult::Scalar(num(1.0))
        );
    }

    #[test]
    fn list_set_rejection_is_atomic() {
        // P0 回归：拒绝的 list_set 不得移除旧元素（先验后改）。
        let mut t = MemoryTree::new(MemoryLimits {
            max_nodes: 100,
            max_bytes: 512,
            max_depth: 8,
            max_string: 8,
        });
        t.map_set(
            0,
            0,
            "l",
            &MemValue::List(vec![num(1.0), num(2.0), num(3.0)]),
        )
        .unwrap();
        let (node, _) = match t.map_get(0, 0, "l").unwrap() {
            ReadResult::Handle { node, kind } => (node, kind),
            other => panic!("期望句柄，得到 {other:?}"),
        };
        let before = t.snapshot();
        let rev = t.revision();
        // 超字节预算的替换值。
        assert!(
            t.list_set(0, node, 1, &MemValue::Str("waaay too long value".into()))
                .is_err()
        );
        assert_eq!(t.snapshot(), before, "拒绝后树必须原样");
        assert_eq!(t.revision(), rev);
        // 非法值替换同样原子。
        assert!(t.list_set(0, node, 0, &MemValue::Num(f64::NAN)).is_err());
        assert_eq!(t.snapshot(), before);
        // 合法替换仍工作。
        t.list_set(0, node, 1, &num(9.0)).unwrap();
        assert_eq!(
            t.to_value(0, node).unwrap(),
            MemValue::List(vec![num(1.0), num(9.0), num(3.0)])
        );
    }

    #[test]
    fn key_length_and_bytes_counted() {
        let mut t = MemoryTree::new(MemoryLimits {
            max_nodes: 100,
            max_bytes: 256,
            max_depth: 4,
            max_string: 8,
        });
        let before = t.snapshot();
        assert!(t.map_set(0, 0, &"k".repeat(9), &num(1.0)).is_err());
        assert_eq!(t.snapshot(), before);
        // 嵌套映射中的长键同样拒绝。
        let deep = MemValue::Map(vec![("abcdefghi".into(), num(1.0))]);
        assert!(t.map_set(0, 0, "m", &deep).is_err());
        assert_eq!(t.snapshot(), before);
        // 键字节计入总预算：短键但字节预算紧张时拒绝。
        let mut t2 = MemoryTree::new(MemoryLimits {
            max_nodes: 100,
            max_bytes: 80,
            max_depth: 4,
            max_string: 64,
        });
        // 根 32B；20B 键 + 32B 标量 = 84B > 80B（键字节计入预算）。
        assert!(t2.map_set(0, 0, &"x".repeat(20), &num(1.0)).is_err());
        // 8B 键：32+8+32 = 72B ≤ 80B，通过。
        t2.map_set(0, 0, &"x".repeat(8), &num(1.0)).unwrap();
    }

    #[test]
    fn node_limit_counts_scalars() {
        // P1 回归：标量叶计入节点数，max_nodes 必须真实成立。
        let mut t = MemoryTree::new(MemoryLimits {
            max_nodes: 4,
            max_bytes: 4096,
            max_depth: 4,
            max_string: 64,
        });
        t.map_set(0, 0, "a", &num(1.0)).unwrap(); // root+1 = 2
        t.map_set(0, 0, "b", &num(2.0)).unwrap(); // 3
        t.map_set(0, 0, "c", &num(3.0)).unwrap(); // 4
        let err = t.map_set(0, 0, "d", &num(4.0)).unwrap_err();
        assert_eq!(err.code, "MEMORY_LIMIT");
        // 替换不增长节点数：合法。
        t.map_set(0, 0, "c", &num(33.0)).unwrap();
    }

    #[test]
    fn robots_entries_managed_by_robot_memory() {
        // P0 回归：robots/<id> 槽被（历史数据）写成标量后，r.memory 原位
        // 修复而不是追加重复键；直接写/删 robots 条目被拒绝。
        let mut t = tree();
        let rm = t.robot_memory(0, 7).unwrap();
        t.map_set(0, rm, "note", &num(1.0)).unwrap();
        let (robots, _) = match t.map_get(0, 0, "robots").unwrap() {
            ReadResult::Handle { node, kind } => (node, kind),
            _ => panic!(),
        };
        assert_eq!(
            t.map_set(0, robots, "7", &num(99.0)).unwrap_err().code,
            "RESERVED_KEY"
        );
        assert_eq!(
            t.map_delete(0, robots, "7").unwrap_err().code,
            "RESERVED_KEY"
        );
        // 键唯一：只有一个 "7"。
        assert_eq!(t.map_keys(0, robots).unwrap(), vec!["7".to_string()]);
        // r.memory 与逐层访问仍是同一节点。
        let via_path = match t.map_get(0, robots, "7").unwrap() {
            ReadResult::Handle { node, .. } => node,
            _ => panic!(),
        };
        assert_eq!(rm, via_path);
        assert_eq!(
            t.map_get(0, via_path, "note").unwrap(),
            ReadResult::Scalar(num(1.0))
        );
    }

    #[test]
    fn list_index_and_kind_errors() {
        let mut t = tree();
        t.map_set(0, 0, "l", &MemValue::List(vec![])).unwrap();
        let (node, _) = handle_of(&mut t, "l");
        assert_eq!(
            t.list_remove(0, node, 0).unwrap_err().code,
            "INDEX_OUT_OF_RANGE"
        );
        assert_eq!(t.map_get(0, node, "x").unwrap_err().code, "WRONG_NODE_KIND");
        assert_eq!(t.list_get(0, 0, 0).unwrap_err().code, "WRONG_NODE_KIND");
    }
}
