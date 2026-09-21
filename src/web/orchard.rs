//! 网页果园超集：3D 沙盘几何数据 + 识别富字段。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/orchard/overview` | POST | `/user/orchard/overview` | `orchard-3d.js:839` |
//! | `/admin/orchard/overview` | POST | `/user/admin/orchard/overview` | `admin.js:887` |
//! | `/citrus-disease-v2` | POST | `/api/citrus-disease-v2` | `analyze.js:196` |
//!
//! 为什么这三个在最严重缺口里（`db/static/WEB_ENDPOINT_MAP.md`）：3D 沙盘要
//! `trees[].position{x,y}` / `terrain_height` / `tag_serial_number` / `latest_sensor` /
//! `latest_diagnosis` + `coordinate_range` + `weather.*`，而 Django 的 `fruit_tree_archive`
//! **无坐标、无地形高度、无 tag_serial_number，也没有传感器记录表**，天气字段零对应。
//!
//! 表：保留 `app_orchard_trees`、`app_tree_sensor_records`；识别富字段写
//! `web_diagnosis_records`（见 `bootstrap/web_tables.rs`），并**双写**契约表
//! `disease_recognition_record` 供 App 读。
//!
//! `/citrus-disease-v2` 放在本文件而不是新开一个：它响应比 Django 多 6 个字段，属识别超集，
//! 与果园/沙盘同族；`src/web.rs` 的六个子文件是并行开发的分界，不为此再加文件。
//!
//! TODO(S2): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
