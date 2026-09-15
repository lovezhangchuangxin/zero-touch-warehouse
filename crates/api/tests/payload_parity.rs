//! payload 字段三方对账（机械锚点）：`ops::OP_FIELDS` ↔ bootstrap.js ↔
//! bootstrap.py 的 rt / rtVoid / _ipc 调用点。op 名与 payload 字段名此前
//! 三处人肉同步——结果码已有 codes_parity 的五处对账，payload 形状无
//! 任何锚点；任一处漂移本测试失败并指出差集。
//!
//! 对账策略：
//! - 双侧调用点出现的 op 必须 ∈ OP_FIELDS（抓未知 op / 拼写漂移）；
//! - OP_FIELDS 的 op 必须 ≤ 双侧并集（抓「服务端已实现、两侧绑定都
//!   没接」的缺口）；单侧缺失是允许的——已知差异：Python 的 `in` 走
//!   `__iter__`（bootstrap.py 容器协议注释），不调 mem.map_has；
//! - 内联字面量的键集必须 == 该 op 的必填 ∪ 可选（多出 / 缺失都抓）；
//! - 动态构造 payload 的调用点（js / py 的 give / drop）：按实参名
//!   回溯字面量（⊇ 必填）并收集点赋值字段，并集 == 全集。注意动态
//!   路径的语义是「字段名出现过」而非「必然发送」——对当前仅有的
//!   可选 box_id 是正确的；若未来必填字段改走动态构造，需加强为
//!   必然性检查；
//! - 调用形态完备性：裸调用名出现处必须紧跟双引号 op 字面量（或
//!   `op` 变量直传），其余写法（单引号 / 换行 / 拼接）一律显式失败
//!   ——否则该调用点会被静默漏检。

use std::collections::BTreeSet;

use ztw_api::ops::OP_FIELDS;

const BOOTSTRAP_JS: &str = include_str!("../bindings/bootstrap.js");
const BOOTSTRAP_PY: &str = include_str!("../bindings/bootstrap.py");

/// 一个调用点：op + payload 形态。
struct CallSite {
    op: String,
    /// 内联对象 / dict 字面量的顶层键集。
    inline: Option<BTreeSet<String>>,
    /// 动态构造：字面量部分键集与后续点赋值键集（inline 为 None 时有效）。
    literal: Option<BTreeSet<String>>,
    assigned: BTreeSet<String>,
}

/// op →（必填 ∪ 可选， 仅必填）。
fn expected(op: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let entry = OP_FIELDS
        .iter()
        .find(|(o, _, _)| *o == op)
        .unwrap_or_else(|| panic!("OP_FIELDS 缺少 {op}"));
    let required: BTreeSet<String> = entry.1.iter().map(|s| s.to_string()).collect();
    let all: BTreeSet<String> = required
        .iter()
        .cloned()
        .chain(entry.2.iter().map(|s| s.to_string()))
        .collect();
    (all, required)
}

/// 引号感知（`"` 与 `'`）的平衡花括号扫描：src 的 byte 下标 start 须为
/// '{'，返回匹配 '}' 的 byte 下标。单引号按字符串处理，避免值里的
/// `}` / `,` 干扰扫描。
fn scan_braces(src: &str, start: usize) -> usize {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut byte = start;
    for c in src[start..].chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
        } else {
            match c {
                '"' | '\'' => quote = Some(c),
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return byte;
                    }
                }
                _ => {}
            }
        }
        byte += c.len_utf8();
    }
    panic!("花括号未闭合（start={start}）");
}

/// 对象 / dict 字面量（含两侧花括号）的顶层键：深度 0 逗号切分，取
/// 冒号前一段为键（js 简写键无冒号则整段即键），去引号。支持多行。
fn literal_keys(lit: &str) -> BTreeSet<String> {
    let inner = &lit[1..lit.len() - 1];
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in inner.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            '{' | '[' | '(' => {
                depth += 1;
                cur.push(c);
            }
            '}' | ']' | ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => tokens.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    tokens.push(cur);
    tokens
        .iter()
        .filter_map(|t| {
            let key = t.split(':').next().unwrap_or("").trim();
            let key = key.trim_matches('"').trim_matches('\'');
            (!key.is_empty()).then(|| key.to_string())
        })
        .collect()
}

/// 裸赋值判定：`=` 开头但排除 `==` / `===`（比较）与 `=>`（箭头）。
fn is_plain_assign(after: &str) -> bool {
    after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>")
}

/// 裸调用名出现处的完备性守卫：所有出现处必须紧跟 `"`（可提取）或
/// 实参恰为 `op` 变量直传（内部转发，叶子调用点仍是字面量）。其余
/// 写法（单引号 / 换行 / 拼接）会被主扫描静默漏掉，这里转成可读失败。
/// 标识符内部子串（如 `.sort(` 之于 `rt(`）按词边界排除；py 侧排除
/// `def _ipc(` 定义，js 侧排除 `function rt(` 定义。
fn assert_call_forms_covered(src: &str, name: &str, py: bool) {
    let call = format!("{name}(");
    let mut from = 0;
    while let Some(rel) = src[from..].find(&call) {
        let at = from + rel;
        from = at + call.len();
        // 词边界：前一个字符是标识符组成部分 → 是别的标识符的子串。
        let in_ident = src[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if in_ident {
            continue;
        }
        if py {
            if at >= 4 && &src[at - 4..at] == "def " {
                continue;
            }
        } else if at >= 9 && &src[at - 9..at] == "function " {
            continue;
        }
        let rest = &src[from..];
        let is_quoted = rest.starts_with('"');
        let is_passthrough = rest.starts_with("op,") || rest.starts_with("op)") || rest == "op";
        if !(is_quoted || is_passthrough) {
            let tail: String = src[at..].chars().take(40).collect();
            panic!(
                "{name}( 出现处未紧跟双引号 op 字面量（也非 op 变量直传）：…{tail}\
                 ——本扫描器只识别 {name}(\"op\", …) 形态，其他写法会静默漏检"
            );
        }
    }
}

/// 提取一侧全部调用点。prefixes 为调用形态前缀（`rt("`、`_ipc("`）。
fn call_sites(src: &str, prefixes: &[&str]) -> Vec<CallSite> {
    // (payload 起始 byte 下标, op)——prefix + op + 引号 + 逗号之后。
    let mut sites: Vec<(usize, String)> = Vec::new();
    for p in prefixes {
        let mut from = 0;
        while let Some(rel) = src[from..].find(p) {
            let at = from + rel;
            let rest = &src[at + p.len()..];
            if let Some(end) = rest.find('"') {
                sites.push((at + p.len() + end + 2, rest[..end].to_string()));
            }
            from = at + p.len();
        }
    }
    sites.sort_by_key(|(at, _)| *at);

    sites
        .iter()
        .map(|(payload_at, op)| {
            // ", " + '{'（内联）或标识符（动态构造）。
            let rest = &src[*payload_at..];
            let body = rest.trim_start();
            let offset = rest.len() - body.len();
            if body.starts_with('{') {
                let open = payload_at + offset;
                let close = scan_braces(src, open);
                CallSite {
                    op: op.clone(),
                    inline: Some(literal_keys(&src[open..=close])),
                    literal: None,
                    assigned: BTreeSet::new(),
                }
            } else {
                // 动态构造：按实参名回溯 `{arg} = {` 字面量并收集点赋值
                // （js `arg.k = …` / py `arg["k"] = …`；比较与注释不算）。
                let arg: String = body
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let ctx = &src[*payload_at..(*payload_at + 30).min(src.len())];
                assert!(
                    !arg.is_empty() && !arg.chars().next().is_some_and(|c| c.is_ascii_digit()),
                    "{op} 调用点的 payload 实参既非内联字面量也非标识符：{ctx}"
                );
                let decl = format!("{arg} = {{");
                let anchor = src[..*payload_at].rfind(&decl).unwrap_or_else(|| {
                    panic!(
                        "{op} 调用点使用变量 {arg}，但之前找不到 `{arg} = {{` 构造字面量\
                         ——变量改名请同步调整本测试的扫描锚点"
                    )
                });
                let open = anchor + format!("{arg} = ").len();
                let close = scan_braces(src, open);
                let mut assigned = BTreeSet::new();
                let mid = &src[close + 1..*payload_at];
                let dot_pat = format!("{arg}.");
                let idx_pat = format!("{arg}[\"");
                let mut from = 0;
                while let Some(rel) = mid[from..].find(&arg) {
                    let at = from + rel;
                    let seg = &mid[at..];
                    // 行前缀含注释符 → 该出现是注释，不算赋值证据。
                    let line_start = mid[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
                    let commented =
                        mid[line_start..at].contains("//") || mid[line_start..at].contains('#');
                    if !commented {
                        let field = if let Some(rest) = seg.strip_prefix(&idx_pat) {
                            // py: arg["k"] =
                            rest.find("\"]").and_then(|q| {
                                let f = &rest[..q];
                                let after = rest[q + 2..].trim_start();
                                (is_plain_assign(after) && !f.is_empty()).then(|| f.to_string())
                            })
                        } else if let Some(rest) = seg.strip_prefix(&dot_pat) {
                            // js: arg.k =
                            let end = rest
                                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                                .unwrap_or(rest.len());
                            let f = &rest[..end];
                            let after = rest[end..].trim_start();
                            (is_plain_assign(after) && !f.is_empty()).then(|| f.to_string())
                        } else {
                            None
                        };
                        if let Some(f) = field {
                            assigned.insert(f);
                        }
                    }
                    from = at + arg.len();
                }
                CallSite {
                    op: op.clone(),
                    inline: None,
                    literal: Some(literal_keys(&src[open..=close])),
                    assigned,
                }
            }
        })
        .collect()
}

#[test]
fn payload_fields_agree_across_bindings() {
    assert_call_forms_covered(BOOTSTRAP_JS, "rt", false);
    assert_call_forms_covered(BOOTSTRAP_JS, "rtVoid", false);
    assert_call_forms_covered(BOOTSTRAP_PY, "_ipc", true);

    let js = call_sites(BOOTSTRAP_JS, &["rt(\"", "rtVoid(\""]);
    let py = call_sites(BOOTSTRAP_PY, &["_ipc(\""]);

    let table: BTreeSet<String> = OP_FIELDS.iter().map(|(o, _, _)| o.to_string()).collect();
    for (name, sites) in [("bootstrap.js", &js), ("bootstrap.py", &py)] {
        let ops: BTreeSet<String> = sites.iter().map(|c| c.op.clone()).collect();
        let extra: Vec<_> = ops.difference(&table).collect();
        assert!(
            extra.is_empty(),
            "{name} 出现 OP_FIELDS 之外的 op（拼写漂移或未登记）：{extra:?}"
        );
    }
    // 单侧缺失允许（如 py 的 in 走 __iter__ 不调 map_has），两侧并集
    // 必须覆盖全部 op——抓「服务端已实现、两侧绑定都没接」的缺口。
    let union: BTreeSet<String> = js.iter().chain(py.iter()).map(|c| c.op.clone()).collect();
    let missing: Vec<_> = table.difference(&union).collect();
    assert!(
        missing.is_empty(),
        "双侧绑定均未调用的 op（OP_FIELDS 与绑定脱节）：{missing:?}"
    );

    for (name, sites) in [("bootstrap.js", &js), ("bootstrap.py", &py)] {
        for site in sites {
            let (all, required) = expected(&site.op);
            if let Some(keys) = &site.inline {
                assert_eq!(
                    keys, &all,
                    "{name} 的 {} 内联字面量字段与 OP_FIELDS 不一致",
                    site.op
                );
            } else {
                let lit = site.literal.clone().expect("动态调用点应有字面量");
                let miss: Vec<_> = required.difference(&lit).collect();
                assert!(
                    miss.is_empty(),
                    "{name} 的 {} 动态 payload 字面量缺必填字段：{miss:?}",
                    site.op
                );
                let mut union = lit;
                union.extend(site.assigned.iter().cloned());
                assert_eq!(
                    union, all,
                    "{name} 的 {} 动态 payload（字面量 + 点赋值）与 OP_FIELDS 不一致",
                    site.op
                );
            }
        }
    }
}
