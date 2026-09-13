// 显示格式化：权威经济值为千分金币十进制字符串，显示层换算成金币
// （docs/architecture/02：f64 不进世界状态，显示转换在消费侧）。

/** 千分金币字符串 → "12.34" 显示（两位小数，向下截断不四舍五入——
 *  与结算侧“逐笔舍入”原则一致，显示值恒不低于真实值语义由调用方解释）。 */
export function fmtMilli(s: string): string {
  const neg = s.startsWith("-");
  const v = BigInt(s.startsWith("-") ? s.slice(1) : s);
  const whole = v / 1000n;
  const frac = (v % 1000n).toString().padStart(3, "0").slice(0, 2);
  return `${neg ? "-" : ""}${whole}.${frac}`;
}

export function fmtPos(x: number, y: number): string {
  return `(${x},${y})`;
}

/** 秒 → "mm:ss"，长局计时。 */
export function fmtTickSeconds(sec: number): string {
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}
