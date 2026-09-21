use base64::Engine;
use sqlx::{PgPool, Row};

use crate::client::OpenRouterClient;

pub(crate) const RECOGNITION_RECORDS_UPLOAD_DIR: &str = "storage/recognition_records";
const RECOGNITION_RECORDS_MEDIA_PREFIX: &str = "/media/recognition_records";

pub(crate) const STORE_COVER_UPLOAD_DIR: &str = "storage/store_covers";
pub(crate) const STORE_COVER_MEDIA_PREFIX: &str = "/store-images";

#[derive(Clone)]
pub struct AppState {
    pub client: OpenRouterClient,
    pub inference: crate::inference::InferenceRuntime,
    pub db: PgPool,
    pub secure_session_cookie: bool,
}

pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn recognition_record_public_path(file_name: &str) -> String {
    format!(
        "{}/{}",
        RECOGNITION_RECORDS_MEDIA_PREFIX,
        file_name.trim_start_matches('/')
    )
}

pub(crate) fn save_recognition_record_image(
    record_id: &str,
    data_url: &str,
) -> anyhow::Result<String> {
    let b64 = if let Some(pos) = data_url.find(',') {
        &data_url[pos + 1..]
    } else {
        data_url
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| anyhow::anyhow!("base64解码失败: {}", e))?;
    let file_name = format!("{}.jpg", record_id);
    let full_path = format!("{}/{}", RECOGNITION_RECORDS_UPLOAD_DIR, file_name);

    std::fs::create_dir_all(RECOGNITION_RECORDS_UPLOAD_DIR)
        .map_err(|e| anyhow::anyhow!("创建目录失败: {}", e))?;
    std::fs::write(&full_path, &bytes).map_err(|e| anyhow::anyhow!("写入图片失败: {}", e))?;

    Ok(recognition_record_public_path(&file_name))
}

/// 保存商城商品封面图，返回公开访问路径 `/store-images/{file_name}`。
pub(crate) fn save_store_cover_image(mime_type: &str, bytes: &[u8]) -> anyhow::Result<String> {
    let extension = match mime_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        _ => return Err(anyhow::anyhow!("仅支持 JPEG / PNG / WebP 图片")),
    };
    if bytes.is_empty() {
        return Err(anyhow::anyhow!("图片内容为空"));
    }
    let file_name = format!("{}.{}", uuid::Uuid::new_v4(), extension);
    let full_path = format!("{STORE_COVER_UPLOAD_DIR}/{file_name}");

    std::fs::create_dir_all(STORE_COVER_UPLOAD_DIR)
        .map_err(|e| anyhow::anyhow!("创建封面目录失败: {}", e))?;
    std::fs::write(&full_path, bytes).map_err(|e| anyhow::anyhow!("写入封面失败: {}", e))?;

    Ok(format!("{STORE_COVER_MEDIA_PREFIX}/{file_name}"))
}

/// 会话 token → username。
///
/// **只认 `auth_token`**（网页会话 `/web/session/*` 与 App 的 Bearer 都写这张表）：
/// 原先「先查 `auth_token`、miss 再查 `app_sessions`」的**双表桥**已随 `app_sessions`
/// 的 DDL 在同一步删掉（S5/G2 的 O4，实测记录见
/// `tests/fixtures/contract/W2_ADMIN_NOTES.md` §5）。
///
/// 注意 `auth_token.key` 是 UUID、`expires_at` 是 `TIMESTAMPTZ`：在 SQL 里用 `now()` 比较，
/// 不要拿 epoch 秒混绑。
pub(crate) async fn lookup_session_username(
    pool: &PgPool,
    token: &str,
) -> Result<Option<String>, sqlx::Error> {
    // `auth_token.key` 是 UUID：非法 UUID 直接当 miss 而不报错。
    if let Ok(key) = uuid::Uuid::parse_str(token.trim()) {
        let row = sqlx::query(
            r#"SELECT u.username
                 FROM auth_token t
                 JOIN "user" u ON u.id = t.user_id
                WHERE t.key = $1 AND t.expires_at > now()
                LIMIT 1"#,
        )
        .bind(key)
        .fetch_optional(pool)
        .await?;

        if let Some(row) = row {
            return Ok(row.try_get::<String, _>("username").ok());
        }
    }

    // S5/G2 的 O4：`app_sessions` 分支已随它的 DDL 一起删除。
    // 现在**只认** `auth_token`（网页会话 `/web/session/*` 与 App 的 Bearer 都写这张表）。
    Ok(None)
}

/// 兼容旧调用点：解析失败一律当 `None`（沿用原有的「吞掉错误」语义）。
pub(crate) async fn username_by_token(state: &AppState, token: &str) -> Option<String> {
    lookup_session_username(&state.db, token)
        .await
        .ok()
        .flatten()
}

pub(crate) fn disease_treatment_text(disease: &str) -> &'static str {
    match disease {
        "黄龙病" => {
            "先杀虫，后砍树：发现病树后，先全园喷洒噻虫嗪、联苯菊酯等药剂杀灭柑橘木虱。间隔3-5天后，将病树连根挖除或砍除，并集中烧毁。砍除时需对树蔸做毁蔸处理（如划十字、涂草甘膦、覆土），防止复发。"
        }
        "沙皮病" => {
            "清园+药剂防治：及时剪除并清理果园内的枯死枝条和落叶，减少病菌来源。在谢花期、幼果期等关键时期，可选用苯醚甲环唑、吡唑醚菌酯、代森锰锌等药剂进行喷雾保护。避免果树遭受冻害或日灼，减少伤口。"
        }
        "溃疡病" => {
            "采用药-剪-药策略：首先使用铜制剂（如噻菌铜、春雷·王铜）全面喷雾杀菌。然后彻底剪除病枝、病叶、病果并集中销毁，修剪工具需消毒。修剪完成后，再喷一次杀菌剂进行保护，7-10天后可再施一次。同时注意防治潜叶蛾等虫媒，减少传播伤口。"
        }
        _ => "暂无治理建议",
    }
}

pub(crate) fn risk_from_disease_name(disease_name: &str) -> &'static str {
    if disease_name == "健康果树" || disease_name == "非果树" {
        "正常"
    } else {
        "高风险"
    }
}
