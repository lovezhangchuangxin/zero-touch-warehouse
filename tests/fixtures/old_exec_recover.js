// 配合 ZTW_FAULT=old_exec_request_first：loop 首个请求被改写为旧执行号，
// 主进程回 EXEC_CLOSED 错误结果、绑定层抛异常；捕获后继续发正常请求并
// 正常完成——验证被拒请求号计入已见对账（不触发 REQUEST_ID_GAP /
// 完成帧 LAST_REQUEST_MISMATCH）。
function loop() {
  try {
    Game.log("first-rejected");
  } catch (e) {
    // EXEC_CLOSED：旧执行可恢复拒绝，吞掉继续。
  }
  Game.log("after-reject-1");
  Game.log("after-reject-2");
}
