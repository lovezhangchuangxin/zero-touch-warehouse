//! 模拟专用 PRNG（docs/architecture/02 确定性规则：固定种子、按子系统
//! 独立分流；禁止系统随机与系统时间）。

/// xoshiro256**：各流自世界种子 + 固定子系统标识派生，状态分别入存档；
/// 状态字节布局随存档格式冻结，变更须迁移旧档。
#[derive(Debug, Clone, PartialEq)]
pub struct Xoshiro256 {
    s: [u64; 4],
}

impl Xoshiro256 {
    /// 自世界种子与子系统标识派生（SplitMix64 混合的标准派生路径）。
    pub fn derive(world_seed: u64, subsystem: &str) -> Xoshiro256 {
        fn splitmix64(x: &mut u64) -> u64 {
            *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        let mut x = world_seed;
        for b in subsystem.bytes() {
            splitmix64(&mut x);
            x ^= b as u64;
        }
        let mut s = [
            splitmix64(&mut x),
            splitmix64(&mut x),
            splitmix64(&mut x),
            splitmix64(&mut x),
        ];
        if s == [0, 0, 0, 0] {
            s[0] = 1; // 全零是 xoshiro 退化态，防御
        }
        Xoshiro256 { s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// [0, n) 取整。装卸口数量是个位数，模偏差可忽略（记录在案）。
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// 状态字（state_hash 与存档用）。
    pub fn state_words(&self) -> [u64; 4] {
        self.s
    }
}
