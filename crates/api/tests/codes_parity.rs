//! 结果码五处同步对账（人工同步点的机械锚点）：ztw_model::codes::ALL ↔
//! bootstrap.js E ↔ bootstrap.py E ↔ game-types GameCode ↔ web codes.ts。
//! 新增 / 删除结果码时五处必须同步落点，任一处漂移本测试失败并指出差集
//! （此前靠评审记忆，NOT_ON_WALL 即五处手改）。

use std::collections::BTreeSet;

use ztw_model::codes;

const BOOTSTRAP_JS: &str = include_str!("../bindings/bootstrap.js");
const BOOTSTRAP_PY: &str = include_str!("../bindings/bootstrap.py");
const GAME_TYPES: &str = include_str!("../../../web/packages/game-types/src/index.d.ts");
const CODES_TS: &str = include_str!("../../../web/apps/game/src/codes.ts");

/// 提取「KEY: "…"」条目表的键集（JS 对象字面量与 Python dict 共用形状：
/// 逗号分隔，键可带引号，非键 token 一律过滤）。
fn object_keys(src: &str, open: &str, close: &str) -> BTreeSet<String> {
    let start = src.find(open).unwrap_or_else(|| panic!("缺少锚点 {open}")) + open.len();
    let end = src[start..]
        .find(close)
        .unwrap_or_else(|| panic!("缺少结束锚点 {close}"))
        + start;
    src[start..end]
        .split(',')
        .map(str::trim)
        .filter(|t| t.contains(':'))
        .map(|t| {
            t.split(':')
                .next()
                .unwrap()
                .trim()
                .trim_matches('"')
                .to_string()
        })
        .filter(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
        .collect()
}

/// GameCode 联合类型成员：按引号切分后隔一取一（全部落在引号内）。
fn game_code_union() -> BTreeSet<String> {
    let start = GAME_TYPES
        .find("export type GameCode =")
        .expect("缺少 GameCode 定义");
    let end = start + GAME_TYPES[start..].find(';').expect("联合类型未闭合");
    GAME_TYPES[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn assert_same(model: &BTreeSet<String>, name: &str, set: &BTreeSet<String>) {
    let extra: Vec<_> = set.difference(model).collect();
    let missing: Vec<_> = model.difference(set).collect();
    assert!(
        extra.is_empty() && missing.is_empty(),
        "{name} 与 codes::ALL 不一致：多出 {extra:?}；缺失 {missing:?}"
    );
}

#[test]
fn code_tables_agree_across_five_sources() {
    let model: BTreeSet<String> = codes::ALL.iter().map(|(k, _)| k.to_string()).collect();
    let js = object_keys(BOOTSTRAP_JS, "const E = Object.freeze({", "})");
    let py = object_keys(BOOTSTRAP_PY, "E = {", "\n}");
    let dts = game_code_union();
    let ts = object_keys(
        CODES_TS,
        "export const CODE_LABELS: Record<string, string> = {",
        "\n};",
    );
    assert_same(&model, "bootstrap.js E", &js);
    assert_same(&model, "bootstrap.py E", &py);
    assert_same(&model, "game-types GameCode", &dts);
    assert_same(&model, "web codes.ts", &ts);
    assert_eq!(model.len(), codes::ALL.len(), "codes::ALL 自身有重复键");
}
