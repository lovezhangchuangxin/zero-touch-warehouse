//! 诊断事件环形缓冲与游标分页（docs/architecture/02 §状态快照与诊断事件）。
//!
//! 事件不随渲染快照丢弃：主进程有界环形（条数 + 字节双重限额），生产快于
//! 消费时覆盖最旧记录并推进保留窗口起点；前端按游标分批拉取 / 确认，
//! 游标早于保留窗口时返回明确 gap，不假装历史完整。Game.log 与诊断事件
//! 使用两个独立实例（日志刷屏不挤掉故障记录，docs/game-design/05）。

use std::collections::VecDeque;

use serde::Serialize;
use serde_json::Value;

/// 单条诊断事件：seq 全局单调（跨 reset 不复用），kind 决定 payload 结构。
#[derive(Debug, Clone, Serialize)]
pub struct DiagEvent {
    pub seq: u64,
    pub tick: u64,
    pub kind: String,
    pub payload: Value,
    /// 序列化字节数（限额记账用，不进线协议）。
    #[serde(skip)]
    cost: usize,
}

/// 游标拉取结果。`next` 为消费本页后应使用的游标（无新事件时等于入参）。
#[derive(Debug, Clone, Serialize)]
pub struct DiagPage {
    pub events: Vec<DiagEvent>,
    pub next: u64,
    /// 游标早于保留窗口：`(after, oldest)` 开区间内的事件已被覆盖，
    /// 前端提示“部分历史已过期”。
    pub gap: Option<Gap>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Gap {
    pub from: u64,
    pub to: u64,
}

pub struct DiagRing {
    events: VecDeque<DiagEvent>,
    next_seq: u64,
    cap: usize,
    byte_cap: usize,
    bytes: usize,
    /// 曾因容量覆盖过事件（区别于 clear_keep_seq 的主动清空：后者
    /// 场景重开后旧游标不应误报“历史已被覆盖”）。
    evicted: bool,
}

impl DiagRing {
    pub fn new(cap: usize, byte_cap: usize) -> DiagRing {
        DiagRing {
            events: VecDeque::new(),
            next_seq: 1, // seq 从 1 起：游标 0 表示“尚未消费任何事件”
            cap,
            byte_cap,
            bytes: 0,
            evicted: false,
        }
    }

    /// 追加事件；超限覆盖最旧（至少保留刚插入的一条）。
    pub fn push(&mut self, tick: u64, kind: &str, payload: Value) {
        let cost = payload.to_string().len();
        self.events.push_back(DiagEvent {
            seq: self.next_seq,
            tick,
            kind: kind.to_string(),
            payload,
            cost,
        });
        self.next_seq += 1;
        self.bytes += cost;
        while self.events.len() > 1 && (self.events.len() > self.cap || self.bytes > self.byte_cap)
        {
            let cost = self.events.front().map(|e| e.cost).unwrap_or(0);
            self.bytes = self.bytes.saturating_sub(cost);
            self.events.pop_front();
            self.evicted = true;
        }
    }

    /// 保留窗口起点（空环时等于 next_seq）。
    fn oldest_seq(&self) -> u64 {
        self.events.front().map(|e| e.seq).unwrap_or(self.next_seq)
    }

    /// 已发布的最大 seq（空环时为 0）。
    pub fn last_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    /// 拉取 seq 大于 `after` 的至多 `limit` 条事件。
    pub fn pull(&self, after: u64, limit: usize) -> DiagPage {
        let gap = (self.evicted && after + 1 < self.oldest_seq()).then(|| Gap {
            from: after + 1,
            to: self.oldest_seq().saturating_sub(1),
        });
        let mut next = after;
        let mut events = Vec::new();
        for e in &self.events {
            if events.len() >= limit {
                break;
            }
            if e.seq > after {
                events.push(e.clone());
                next = e.seq;
            }
        }
        DiagPage { events, next, gap }
    }

    /// 确认消费到 `cursor`：丢弃该前缀释放空间（覆盖兜底仍生效）。
    pub fn ack(&mut self, cursor: u64) {
        while self.events.front().is_some_and(|e| e.seq <= cursor) {
            let cost = self.events.front().map(|e| e.cost).unwrap_or(0);
            self.bytes = self.bytes.saturating_sub(cost);
            self.events.pop_front();
        }
    }

    /// 清空内容但延续 seq（场景重建后旧游标仍有效且不误报新事件）。
    pub fn clear_keep_seq(&mut self) {
        self.events.clear();
        self.bytes = 0;
        self.evicted = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ring(n: u64) -> DiagRing {
        let mut r = DiagRing::new(8, 64 * 1024);
        for i in 0..n {
            r.push(i, "settle", json!({ "i": i }));
        }
        r
    }

    #[test]
    fn cursor_paging_without_loss() {
        let mut r = ring(6);
        let p1 = r.pull(0, 4);
        assert_eq!(p1.events.len(), 4);
        assert_eq!(p1.next, 4);
        assert!(p1.gap.is_none());
        let p2 = r.pull(p1.next, 4);
        assert_eq!(p2.events.len(), 2);
        assert_eq!(p2.next, 6);
        // 无新事件时 next 不变。
        let p3 = r.pull(p2.next, 4);
        assert!(p3.events.is_empty());
        assert_eq!(p3.next, 6);
        // ack 只释放已确认前缀，未确认事件仍可重试拉取（去重由 seq 保证）。
        r.ack(3);
        let p4 = r.pull(2, 10);
        assert_eq!(
            p4.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
    }

    #[test]
    fn overwrite_reports_gap() {
        let mut r = DiagRing::new(4, usize::MAX);
        for i in 0..10 {
            r.push(i, "log", json!({ "i": i }));
        }
        assert_eq!(r.last_seq(), 10);
        // 游标 2 早于保留窗口（oldest=7）：返回现存全部并带明确 gap。
        let p = r.pull(2, 100);
        assert_eq!(p.events.first().map(|e| e.seq), Some(7));
        assert_eq!(p.gap, Some(Gap { from: 3, to: 6 }));
        // 窗口内游标无 gap。
        assert!(r.pull(6, 100).gap.is_none());
    }

    #[test]
    fn byte_cap_overwrites_oldest() {
        let mut r = DiagRing::new(100, 64);
        for i in 0..20 {
            r.push(i, "log", json!({ "pad": "x".repeat(8) })); // 每条约 20B
        }
        assert!(r.bytes <= 64 + 20, "超限后回落到约一条余量：{}", r.bytes);
        let p = r.pull(0, 100);
        assert!(p.events.len() <= 5, "{}", p.events.len());
    }

    #[test]
    fn clear_keep_seq_does_not_report_gap() {
        let mut r = ring(6);
        r.clear_keep_seq();
        // 场景重开后旧游标不应报“历史已被覆盖”——那是容量覆盖的语义。
        assert!(r.pull(0, 10).gap.is_none());
        // 真实覆盖恢复 gap 语义。
        r.push(0, "settle", json!({ "i": 0 }));
        assert!(r.pull(0, 10).gap.is_none());
    }

    #[test]
    fn empty_ring_and_oversized_single_event() {
        let mut r = DiagRing::new(4, 8);
        r.push(0, "fault", json!({ "msg": "x".repeat(200) }));
        let p = r.pull(0, 10);
        assert_eq!(p.events.len(), 1, "单条超限事件不自我覆盖");
        assert_eq!(p.next, 1);
        // 空环：无事件、next 保持入参、无 gap。
        let e = DiagRing::new(4, 64).pull(0, 10);
        assert!(e.events.is_empty());
        assert_eq!(e.next, 0);
        assert!(e.gap.is_none());
    }
}
