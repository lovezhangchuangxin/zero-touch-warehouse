// 配合 ZTW_FAULT=old_exec_request / stale_epoch：loop 首个请求被改写为
// 旧执行号 / 旧代次，主进程应拒绝（EXEC_CLOSED / STALE_EPOCH）。
function loop() {
  Game.log("hello");
}
