//! 单文件只保留 `/store-images/*` 那条**隐式静态通道**。
//!
//! 模块名留着的理由是路由仍叫 `/store-images/{file_name}`、目录仍是 `storage/store_covers/`；
//! 同文件里的自研商城列表接口（`storefront_handler`，读 `store_products`）已随 G2 的
//! `/api/store/products` 路由删除——商品数据面在 S4 之后是契约表 `citrus_product`
//! （前端打 `/api/products` 与 `/web/admin/store/products*`）。
//!
//! **不要**因为「只剩一个 handler」就整文件删掉：`citrus_product.cover_image_url` 里存的是
//! `/store-images/<uuid>.jpg` 这类**数据驱动的路径字符串**，App 与网页都直接 `<img src>` 取它，
//! 没有 JS fetch 会暴露它的死亡。三条同类通道另两条在 `handlers_core/media.rs`。

use axum::{
    body::Body,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

fn valid_cover_file_name(file_name: &str) -> bool {
    let extension = file_name.rsplit('.').next().unwrap_or("");
    matches!(extension, "jpg" | "jpeg" | "png" | "webp")
        && file_name.len() <= 64
        && file_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// 公开访问商城商品封面图（商品数据本身对游客公开）。
pub(crate) async fn store_cover_image_handler(Path(file_name): Path<String>) -> Response {
    if !valid_cover_file_name(&file_name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let storage_path = format!(
        "{}/{}",
        crate::server::shared::STORE_COVER_UPLOAD_DIR,
        file_name
    );
    let bytes = match tokio::fs::read(&storage_path).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let content_type = if file_name.ends_with(".png") {
        "image/png"
    } else if file_name.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        Body::from(bytes),
    )
        .into_response()
}
