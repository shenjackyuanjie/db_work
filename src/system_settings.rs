use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

const SETTINGS_ROW_ID: i16 = 1;
const DEFAULT_INVITE_TTL_SECONDS: i64 = 24 * 60 * 60;
const DEFAULT_CONFIDENCE_THRESHOLD: f64 = 0.75;
const DEFAULT_LOG_RETENTION_DAYS: i32 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSettings {
    pub open_registration: bool,
    pub invite_bypass_enabled: bool,
    pub maintenance_mode: bool,
    pub default_invite_ttl_seconds: i64,
    pub confidence_threshold: f64,
    pub log_retention_days: i32,
    pub updated_at: u64,
    pub updated_by: Option<String>,
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            open_registration: true,
            invite_bypass_enabled: true,
            maintenance_mode: false,
            default_invite_ttl_seconds: DEFAULT_INVITE_TTL_SECONDS,
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
            log_retention_days: DEFAULT_LOG_RETENTION_DAYS,
            updated_at: now_millis(),
            updated_by: None,
        }
    }
}

impl SystemSettings {
    pub fn sanitized(mut self) -> Self {
        self.default_invite_ttl_seconds = self.default_invite_ttl_seconds.clamp(3600, 365 * 24 * 60 * 60);
        self.confidence_threshold = self.confidence_threshold.clamp(0.30, 0.99);
        self.log_retention_days = self.log_retention_days.clamp(1, 365);
        self
    }

    pub fn confidence_threshold_percent(&self) -> f64 {
        self.confidence_threshold * 100.0
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub async fn ensure_default_settings(pool: &PgPool) -> anyhow::Result<()> {
    let defaults = SystemSettings::default().sanitized();

    sqlx::query(
        r#"INSERT INTO app_system_settings (
            id,
            open_registration,
            invite_bypass_enabled,
            maintenance_mode,
            default_invite_ttl_seconds,
            confidence_threshold,
            log_retention_days,
            updated_at,
            updated_by
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(SETTINGS_ROW_ID)
    .bind(defaults.open_registration)
    .bind(defaults.invite_bypass_enabled)
    .bind(defaults.maintenance_mode)
    .bind(defaults.default_invite_ttl_seconds)
    .bind(defaults.confidence_threshold)
    .bind(defaults.log_retention_days)
    .bind(defaults.updated_at as i64)
    .bind(defaults.updated_by)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn load_system_settings(pool: &PgPool) -> anyhow::Result<SystemSettings> {
    ensure_default_settings(pool).await?;

    let row = sqlx::query(
        r#"SELECT
            open_registration,
            invite_bypass_enabled,
            maintenance_mode,
            default_invite_ttl_seconds,
            confidence_threshold,
            log_retention_days,
            updated_at,
            updated_by
        FROM app_system_settings
        WHERE id = $1
        LIMIT 1"#,
    )
    .bind(SETTINGS_ROW_ID)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(SystemSettings::default());
    };

    Ok(SystemSettings {
        open_registration: row.try_get::<bool, _>("open_registration").unwrap_or(true),
        invite_bypass_enabled: row
            .try_get::<bool, _>("invite_bypass_enabled")
            .unwrap_or(true),
        maintenance_mode: row.try_get::<bool, _>("maintenance_mode").unwrap_or(false),
        default_invite_ttl_seconds: row
            .try_get::<i64, _>("default_invite_ttl_seconds")
            .unwrap_or(DEFAULT_INVITE_TTL_SECONDS),
        confidence_threshold: row
            .try_get::<f64, _>("confidence_threshold")
            .unwrap_or(DEFAULT_CONFIDENCE_THRESHOLD),
        log_retention_days: row
            .try_get::<i32, _>("log_retention_days")
            .unwrap_or(DEFAULT_LOG_RETENTION_DAYS),
        updated_at: row.try_get::<i64, _>("updated_at").unwrap_or(0).max(0) as u64,
        updated_by: row.try_get::<Option<String>, _>("updated_by").unwrap_or(None),
    }
    .sanitized())
}

pub async fn update_system_settings(
    pool: &PgPool,
    settings: SystemSettings,
    updated_by: Option<&str>,
) -> anyhow::Result<SystemSettings> {
    let sanitized = settings.sanitized();
    let updated_at = now_millis() as i64;

    sqlx::query(
        r#"UPDATE app_system_settings
        SET
            open_registration = $1,
            invite_bypass_enabled = $2,
            maintenance_mode = $3,
            default_invite_ttl_seconds = $4,
            confidence_threshold = $5,
            log_retention_days = $6,
            updated_at = $7,
            updated_by = $8
        WHERE id = $9"#,
    )
    .bind(sanitized.open_registration)
    .bind(sanitized.invite_bypass_enabled)
    .bind(sanitized.maintenance_mode)
    .bind(sanitized.default_invite_ttl_seconds)
    .bind(sanitized.confidence_threshold)
    .bind(sanitized.log_retention_days)
    .bind(updated_at)
    .bind(updated_by.map(str::to_string))
    .bind(SETTINGS_ROW_ID)
    .execute(pool)
    .await?;

    load_system_settings(pool).await
}

pub async fn append_audit_log(
    pool: &PgPool,
    log_type: &str,
    actor_username: Option<&str>,
    message: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO app_admin_audit_logs (id, log_type, actor_username, message, created_at)
        VALUES ($1, $2, $3, $4, $5)"#,
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(log_type)
    .bind(actor_username.map(str::to_string))
    .bind(message)
    .bind(now_millis() as i64)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn prune_audit_logs(pool: &PgPool, retention_days: i32) -> anyhow::Result<u64> {
    let safe_days = retention_days.clamp(1, 365) as i64;
    let cutoff = now_millis() as i64 - safe_days * 24 * 60 * 60 * 1000;

    let result = sqlx::query("DELETE FROM app_admin_audit_logs WHERE created_at < $1")
        .bind(cutoff)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}