// 配合 dedup_cache_bytes 小值配置 + ZTW_FAULT=dup_request:1：单条超长
// 日志的指纹+结果远超字节预算，不入缓存；注入的重发无缓存可回放，
// 预期主进程按 DUP_REQUEST_MISMATCH 协议故障终止宿主（优雅降级）。
function loop() {
  Game.log("L".repeat(1600));
}
