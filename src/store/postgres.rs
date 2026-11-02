//! Durable metadata adapter (FR-001, FR-002; research R1–R7).
//!
//! PostgreSQL is the system of record. This module implements the identity half
//! of the repository surface first: users, sessions, recovery codes,
//! organisations, organisation membership, and invitations. Project, telemetry,
//! and operations SQL build on the same pool and row-mapping conventions.
//!
//! Only hashes are ever written for passwords, sessions, recovery codes, and
//! invitation tokens. Nothing here serialises a secret.
//!
//! The adapter is wired into `AppState` by the seam task (T018/T019); until that
//! lands, these methods are exercised by the ignored database tests only.
#![allow(dead_code)]

use crate::models::*;
use chrono::{DateTime, Duration, Utc};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;
use std::time::Duration as StdDuration;
use uuid::Uuid;

/// A pool plus the identity queries that make it a repository.
#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

const USER_COLUMNS: &str =
    "id, email::text AS email, name, password_hash, is_operator, disabled_at, locked_at, \
     failed_sign_in_count";

const ORG_COLUMNS: &str =
    "id, name, COALESCE(slug, '') AS slug, sso_provider, created_by, is_demo, created_at";

const INVITATION_COLUMNS: &str = "id, org_id, kind, project_id, email::text AS email, role, \
     token_hash, created_by, expires_at, accepted_at, accepted_user_id, revoked_at";

fn db(error: sqlx::Error) -> String {
    // Driver details stay in the log; callers receive an actionable summary.
    tracing::warn!(%error, "postgres identity statement failed");
    "the metadata store could not complete the request; retry shortly".to_string()
}

fn user_from_row(row: &PgRow) -> Result<User, sqlx::Error> {
    let failed: i32 = row.try_get("failed_sign_in_count")?;
    Ok(User {
        id: row.try_get("id")?,
        email: row.try_get("email")?,
        name: row.try_get("name")?,
        password_hash: row.try_get("password_hash")?,
        is_operator: row.try_get("is_operator")?,
        disabled_at: row.try_get("disabled_at")?,
        locked_at: row.try_get("locked_at")?,
        failed_sign_in_count: failed.max(0) as u32,
    })
}

fn organisation_from_row(row: &PgRow) -> Result<Organisation, sqlx::Error> {
    Ok(Organisation {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        sso_provider: row.try_get("sso_provider")?,
        created_by: row.try_get("created_by")?,
        is_demo: row.try_get("is_demo")?,
        created_at: row.try_get("created_at")?,
    })
}

fn parse_role(value: Option<&str>) -> Option<Role> {
    match value? {
        "owner" => Some(Role::Owner),
        "admin" => Some(Role::Admin),
        "triager" => Some(Role::Triager),
        "viewer" => Some(Role::Viewer),
        _ => None,
    }
}

fn parse_org_role(value: &str) -> Option<OrgRole> {
    match value {
        "org_admin" => Some(OrgRole::Admin),
        "org_member" => Some(OrgRole::Member),
        _ => None,
    }
}

fn parse_invitation_kind(value: &str) -> Option<InvitationKind> {
    match value {
        "project_invite" => Some(InvitationKind::ProjectInvite),
        "org_admin_setup" => Some(InvitationKind::OrgAdminSetup),
        _ => None,
    }
}

fn invitation_from_row(row: &PgRow) -> Result<Invitation, sqlx::Error> {
    let kind: String = row.try_get("kind")?;
    let role: Option<String> = row.try_get("role")?;
    Ok(Invitation {
        id: row.try_get("id")?,
        org_id: row.try_get("org_id")?,
        kind: parse_invitation_kind(&kind).unwrap_or(InvitationKind::ProjectInvite),
        project_id: row.try_get("project_id")?,
        email: row.try_get("email")?,
        role: parse_role(role.as_deref()),
        token_hash: row.try_get("token_hash")?,
        created_by: row.try_get("created_by")?,
        expires_at: row.try_get("expires_at")?,
        accepted_at: row.try_get("accepted_at")?,
        accepted_user_id: row.try_get("accepted_user_id")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}

impl PostgresStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Connect with the bounded retry from [`crate::schema::connect`].
    pub async fn connect(database_url: &str) -> Result<Self, String> {
        Ok(Self::new(crate::schema::connect(database_url).await?))
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn kind(&self) -> &'static str {
        "postgres"
    }

    pub fn is_durable(&self) -> bool {
        true
    }

    /// `SELECT 1` with a two-second bound (FR-004, SC-004).
    pub async fn health(&self) -> bool {
        matches!(
            tokio::time::timeout(
                StdDuration::from_secs(2),
                sqlx::query("SELECT 1").execute(&self.pool)
            )
            .await,
            Ok(Ok(_))
        )
    }

    // --- users -------------------------------------------------------------

    pub async fn user_by_email(&self, email: &str) -> Result<Option<User>, String> {
        let row = sqlx::query(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE email = $1::citext"
        ))
        .bind(email.trim())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref().map(user_from_row).transpose().map_err(db)
    }

    pub async fn user_by_id(&self, user_id: Uuid) -> Result<Option<User>, String> {
        let row = sqlx::query(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        row.as_ref().map(user_from_row).transpose().map_err(db)
    }

    /// The unique partial index enforces a single operator.
    pub async fn operator_exists(&self) -> Result<bool, String> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE is_operator)")
            .fetch_one(&self.pool)
            .await
            .map_err(db)
    }

    pub async fn insert_user(&self, user: &User) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, is_operator, disabled_at, \
             locked_at, failed_sign_in_count) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.password_hash)
        .bind(user.is_operator)
        .bind(user.disabled_at)
        .bind(user.locked_at)
        .bind(user.failed_sign_in_count as i32)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn set_user_password(
        &self,
        user_id: Uuid,
        password_hash: &str,
    ) -> Result<(), String> {
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user_id)
            .bind(password_hash)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn set_user_disabled(&self, user_id: Uuid, disabled: bool) -> Result<(), String> {
        sqlx::query(
            "UPDATE users SET disabled_at = CASE WHEN $2 THEN COALESCE(disabled_at, now()) \
             ELSE NULL END WHERE id = $1",
        )
        .bind(user_id)
        .bind(disabled)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Count a consecutive failure and lock at the threshold. Returns true when
    /// this failure locked the account (FR-032).
    pub async fn record_failed_sign_in(&self, user_id: Uuid) -> Result<bool, String> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let row = sqlx::query("SELECT failed_sign_in_count FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
        let Some(row) = row else {
            tx.rollback().await.ok();
            return Ok(false);
        };
        let failed: i32 = row.try_get("failed_sign_in_count").map_err(db)?;
        let next = failed.saturating_add(1);
        let locked = next as u32 >= crate::store::MAX_FAILED_SIGN_INS;
        sqlx::query(
            "UPDATE users SET failed_sign_in_count = $2, \
             locked_at = CASE WHEN $3 THEN COALESCE(locked_at, now()) ELSE locked_at END \
             WHERE id = $1",
        )
        .bind(user_id)
        .bind(next)
        .bind(locked)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(locked)
    }

    pub async fn clear_failed_sign_ins(&self, user_id: Uuid) -> Result<(), String> {
        sqlx::query("UPDATE users SET failed_sign_in_count = 0 WHERE id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Operator unlock: clears both the lock and the counter.
    pub async fn unlock_user(&self, user_id: Uuid) -> Result<bool, String> {
        let result = sqlx::query(
            "UPDATE users SET locked_at = NULL, failed_sign_in_count = 0 WHERE id = $1",
        )
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    /// Locked accounts, newest lock first.
    pub async fn locked_users(&self) -> Result<Vec<User>, String> {
        let rows = sqlx::query(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE locked_at IS NOT NULL \
             ORDER BY locked_at DESC, email"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(user_from_row)
            .collect::<Result<_, _>>()
            .map_err(db)
    }

    // --- sessions ----------------------------------------------------------

    pub async fn issue_session(&self, user_id: Uuid, ttl_days: i64) -> Result<String, String> {
        let token = crate::auth::new_token();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, token_hash, created_at, expires_at, last_seen_at) \
             VALUES ($1, $2, $3, $4, $5, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(crate::auth::hash_token(&token))
        .bind(now)
        .bind(now + Duration::days(ttl_days))
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(token)
    }

    /// Resolve a session, enforcing the absolute and idle limits and refreshing
    /// its last activity (FR-033). Expired rows are deleted.
    pub async fn user_for_token(
        &self,
        token: &str,
        idle_hours: i64,
    ) -> Result<Option<Uuid>, String> {
        let hash = crate::auth::hash_token(token);
        let row = sqlx::query(
            "SELECT user_id, expires_at, COALESCE(last_seen_at, created_at) AS last_seen_at \
             FROM sessions WHERE token_hash = $1",
        )
        .bind(&hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let user_id: Uuid = row.try_get("user_id").map_err(db)?;
        let expires_at: DateTime<Utc> = row.try_get("expires_at").map_err(db)?;
        let last_seen_at: DateTime<Utc> = row.try_get("last_seen_at").map_err(db)?;
        let now = Utc::now();
        let idle_expired = idle_hours > 0 && now - last_seen_at > Duration::hours(idle_hours);
        if expires_at <= now || idle_expired {
            sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
                .bind(&hash)
                .execute(&self.pool)
                .await
                .map_err(db)?;
            return Ok(None);
        }
        sqlx::query("UPDATE sessions SET last_seen_at = $2 WHERE token_hash = $1")
            .bind(&hash)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(Some(user_id))
    }

    pub async fn revoke_session(&self, token: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(crate::auth::hash_token(token))
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    /// End every session for the account except `keep_hash`.
    pub async fn revoke_user_sessions_except(
        &self,
        user_id: Uuid,
        keep_hash: Option<&str>,
    ) -> Result<(), String> {
        sqlx::query(
            "DELETE FROM sessions WHERE user_id = $1 AND ($2::text IS NULL OR token_hash <> $2)",
        )
        .bind(user_id)
        .bind(keep_hash)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Delete expired sessions. Returns the number removed.
    pub async fn sweep_sessions(&self) -> Result<u64, String> {
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    // --- recovery codes ----------------------------------------------------

    pub async fn has_recovery_codes(&self, user_id: Uuid) -> Result<bool, String> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM recovery_codes WHERE user_id = $1)")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(db)
    }

    /// Revoke every unused code and issue a fresh set in one transaction. The
    /// plaintext codes are returned once; only hashes are stored (FR-017).
    pub async fn issue_recovery_codes(
        &self,
        user_id: Uuid,
        count: usize,
    ) -> Result<Vec<String>, String> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "UPDATE recovery_codes SET revoked_at = now() \
             WHERE user_id = $1 AND used_at IS NULL AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        let mut plain = Vec::with_capacity(count);
        for _ in 0..count {
            let code = crate::auth::new_recovery_code();
            sqlx::query("INSERT INTO recovery_codes (id, user_id, code_hash) VALUES ($1, $2, $3)")
                .bind(Uuid::new_v4())
                .bind(user_id)
                .bind(crate::auth::hash_token(&code))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            plain.push(code);
        }
        tx.commit().await.map_err(db)?;
        Ok(plain)
    }

    /// Look up an unused, unrevoked code for an account by its hash.
    pub async fn usable_recovery_code(
        &self,
        user_id: Uuid,
        code_hash: &str,
    ) -> Result<Option<RecoveryCode>, String> {
        let row = sqlx::query(
            "SELECT id, user_id, code_hash, created_at, used_at, revoked_at FROM recovery_codes \
             WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(code_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(RecoveryCode {
            id: row.try_get("id").map_err(db)?,
            user_id: row.try_get("user_id").map_err(db)?,
            code_hash: row.try_get("code_hash").map_err(db)?,
            created_at: row.try_get("created_at").map_err(db)?,
            used_at: row.try_get("used_at").map_err(db)?,
            revoked_at: row.try_get("revoked_at").map_err(db)?,
        }))
    }

    pub async fn mark_recovery_code_used(&self, code_id: Uuid) -> Result<(), String> {
        sqlx::query("UPDATE recovery_codes SET used_at = now() WHERE id = $1 AND used_at IS NULL")
            .bind(code_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    // --- organisations -----------------------------------------------------

    pub async fn organisation_by_id(&self, org_id: Uuid) -> Result<Option<Organisation>, String> {
        let row = sqlx::query(&format!(
            "SELECT {ORG_COLUMNS} FROM organisations WHERE id = $1"
        ))
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref()
            .map(organisation_from_row)
            .transpose()
            .map_err(db)
    }

    pub async fn organisation_by_slug(&self, slug: &str) -> Result<Option<Organisation>, String> {
        let row = sqlx::query(&format!(
            "SELECT {ORG_COLUMNS} FROM organisations WHERE lower(slug) = lower($1)"
        ))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref()
            .map(organisation_from_row)
            .transpose()
            .map_err(db)
    }

    /// Allocate a slug with the same suffixing rule as the in-memory adapter.
    pub async fn unique_org_slug(&self, name: &str) -> Result<String, String> {
        let base = slugify(name);
        let taken: Vec<String> =
            sqlx::query_scalar("SELECT lower(slug) FROM organisations WHERE slug IS NOT NULL")
                .fetch_all(&self.pool)
                .await
                .map_err(db)?;
        if !taken.iter().any(|slug| slug == &base) {
            return Ok(base);
        }
        for suffix in 2..10_000u32 {
            let candidate = format!("{base}-{suffix}");
            if !taken.iter().any(|slug| slug == &candidate) {
                return Ok(candidate);
            }
        }
        Ok(format!("{base}-{}", Uuid::new_v4().simple()))
    }

    pub async fn insert_organisation(&self, organisation: &Organisation) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO organisations (id, name, slug, sso_provider, created_by, is_demo, \
             created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(organisation.id)
        .bind(&organisation.name)
        .bind(&organisation.slug)
        .bind(&organisation.sso_provider)
        .bind(organisation.created_by)
        .bind(organisation.is_demo)
        .bind(organisation.created_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn org_role_for(
        &self,
        user_id: Uuid,
        org_id: Uuid,
    ) -> Result<Option<OrgRole>, String> {
        let role: Option<String> =
            sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
                .bind(org_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(role.as_deref().and_then(parse_org_role))
    }

    pub async fn upsert_org_member(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        role: OrgRole,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO org_members (org_id, user_id, role) VALUES ($1, $2, $3) \
             ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role",
        )
        .bind(org_id)
        .bind(user_id)
        .bind(role.to_string())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    // --- invitations -------------------------------------------------------

    /// Insert with a hashed token. The plaintext token is the caller's to show
    /// exactly once.
    pub async fn insert_invitation(
        &self,
        invitation: &Invitation,
        token_hash: &str,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO invitations (id, org_id, kind, project_id, email, role, token_hash, \
             created_by, expires_at, accepted_at, accepted_user_id, revoked_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(invitation.id)
        .bind(invitation.org_id)
        .bind(invitation.kind.to_string())
        .bind(invitation.project_id)
        .bind(&invitation.email)
        .bind(invitation.role.map(|role| role.to_string()))
        .bind(token_hash)
        .bind(invitation.created_by)
        .bind(invitation.expires_at)
        .bind(invitation.accepted_at)
        .bind(invitation.accepted_user_id)
        .bind(invitation.revoked_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn invitation_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<Invitation>, String> {
        let row = sqlx::query(&format!(
            "SELECT {INVITATION_COLUMNS} FROM invitations WHERE token_hash = $1"
        ))
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref()
            .map(invitation_from_row)
            .transpose()
            .map_err(db)
    }

    pub async fn invitation_by_id(
        &self,
        invitation_id: Uuid,
    ) -> Result<Option<Invitation>, String> {
        let row = sqlx::query(&format!(
            "SELECT {INVITATION_COLUMNS} FROM invitations WHERE id = $1"
        ))
        .bind(invitation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref()
            .map(invitation_from_row)
            .transpose()
            .map_err(db)
    }

    pub async fn mark_invitation_accepted(
        &self,
        invitation_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE invitations SET accepted_at = now(), accepted_user_id = $2 \
             WHERE id = $1 AND accepted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(invitation_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn mark_invitation_revoked(&self, invitation_id: Uuid) -> Result<(), String> {
        sqlx::query(
            "UPDATE invitations SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(invitation_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One database-backed test lives here at a time so ignored runs never
    /// interleave on the same shared schema.
    async fn test_store() -> Option<PostgresStore> {
        let url = std::env::var("SCOUT_TEST_DATABASE_URL").ok()?;
        let store = PostgresStore::connect(&url).await.expect("connect");
        crate::schema::prepare(store.pool())
            .await
            .expect("apply migrations");
        sqlx::query("TRUNCATE users, organisations, sessions, recovery_codes, invitations CASCADE")
            .execute(store.pool())
            .await
            .expect("reset identity tables");
        Some(store)
    }

    #[tokio::test]
    #[ignore = "requires SCOUT_TEST_DATABASE_URL"]
    async fn postgres_identity_surface_round_trips() {
        let Some(store) = test_store().await else {
            return;
        };

        // Users and the single operator.
        let hash = crate::auth::hash_password("operator-secret").expect("hash");
        let operator = User {
            id: Uuid::new_v4(),
            email: "ops@test.local".into(),
            name: "Operator".into(),
            password_hash: Some(hash.clone()),
            is_operator: true,
            disabled_at: None,
            locked_at: None,
            failed_sign_in_count: 0,
        };
        store.insert_user(&operator).await.expect("insert operator");
        assert!(store.operator_exists().await.expect("operator check"));
        let loaded = store
            .user_by_email("OPS@test.local")
            .await
            .expect("lookup")
            .expect("operator row");
        assert_eq!(loaded.id, operator.id);
        assert_eq!(loaded.password_hash.as_deref(), Some(hash.as_str()));
        // The unique partial index keeps a second operator out.
        let second = User {
            id: Uuid::new_v4(),
            email: "ops2@test.local".into(),
            name: "Second".into(),
            password_hash: None,
            is_operator: true,
            disabled_at: None,
            locked_at: None,
            failed_sign_in_count: 0,
        };
        assert!(store.insert_user(&second).await.is_err());

        // Lockout: five consecutive failures, then an unlock resets the counter.
        for expected in 1..=crate::store::MAX_FAILED_SIGN_INS {
            let locked = store
                .record_failed_sign_in(operator.id)
                .await
                .expect("count failure");
            assert_eq!(locked, expected == crate::store::MAX_FAILED_SIGN_INS);
        }
        let locked = store.user_by_id(operator.id).await.unwrap().unwrap();
        assert!(locked.locked_at.is_some());
        assert_eq!(
            locked.failed_sign_in_count,
            crate::store::MAX_FAILED_SIGN_INS
        );
        assert_eq!(store.locked_users().await.expect("list locked").len(), 1);
        assert!(store.unlock_user(operator.id).await.expect("unlock"));
        let unlocked = store.user_by_id(operator.id).await.unwrap().unwrap();
        assert!(unlocked.locked_at.is_none());
        assert_eq!(unlocked.failed_sign_in_count, 0);

        // Sessions honour absolute and idle expiry and refresh activity.
        let token = store.issue_session(operator.id, 7).await.expect("session");
        assert_eq!(
            store
                .user_for_token(&token, 12)
                .await
                .expect("resolve session"),
            Some(operator.id)
        );
        sqlx::query(
            "UPDATE sessions SET last_seen_at = now() - interval '13 hours' WHERE user_id = $1",
        )
        .bind(operator.id)
        .execute(store.pool())
        .await
        .expect("age session");
        assert_eq!(
            store.user_for_token(&token, 12).await.expect("idle check"),
            None
        );
        let token = store.issue_session(operator.id, 7).await.expect("session");
        sqlx::query(
            "UPDATE sessions SET expires_at = now() - interval '1 second' WHERE user_id = $1",
        )
        .bind(operator.id)
        .execute(store.pool())
        .await
        .expect("expire session");
        assert_eq!(
            store
                .user_for_token(&token, 12)
                .await
                .expect("absolute check"),
            None
        );
        assert_eq!(store.sweep_sessions().await.expect("sweep"), 0);

        // Recovery codes: revoke-then-issue, usable lookup, single use.
        assert!(!store.has_recovery_codes(operator.id).await.expect("empty"));
        let first = store
            .issue_recovery_codes(operator.id, 10)
            .await
            .expect("issue codes");
        assert_eq!(first.len(), 10);
        assert!(store.has_recovery_codes(operator.id).await.expect("codes"));
        let first_id = store
            .usable_recovery_code(operator.id, &crate::auth::hash_token(&first[0]))
            .await
            .expect("lookup code")
            .expect("usable code")
            .id;
        let second = store
            .issue_recovery_codes(operator.id, 10)
            .await
            .expect("reissue codes");
        assert!(
            store
                .usable_recovery_code(operator.id, &crate::auth::hash_token(&first[0]))
                .await
                .expect("old code")
                .is_none(),
            "reissuing revokes the prior unused set"
        );
        assert!(store
            .usable_recovery_code(operator.id, &crate::auth::hash_token(&second[0]))
            .await
            .expect("new code")
            .is_some());
        assert_ne!(first_id, Uuid::nil());
        store
            .mark_recovery_code_used(first_id)
            .await
            .expect("consume code");

        // Organisations, slugs, membership.
        let name = "Acme Identity";
        let slug = store.unique_org_slug(name).await.expect("slug");
        assert_eq!(slug, "acme-identity");
        let org = Organisation {
            id: Uuid::new_v4(),
            name: name.into(),
            slug: slug.clone(),
            sso_provider: None,
            created_by: Some(operator.id),
            is_demo: false,
            created_at: Utc::now(),
        };
        store.insert_organisation(&org).await.expect("insert org");
        assert_eq!(
            store
                .organisation_by_slug("ACME-IDENTITY")
                .await
                .expect("slug lookup")
                .expect("org")
                .id,
            org.id
        );
        assert_eq!(
            store.unique_org_slug(name).await.expect("next slug"),
            "acme-identity-2"
        );
        assert_eq!(store.org_role_for(operator.id, org.id).await.unwrap(), None);
        store
            .upsert_org_member(org.id, operator.id, OrgRole::Admin)
            .await
            .expect("membership");
        assert_eq!(
            store.org_role_for(operator.id, org.id).await.unwrap(),
            Some(OrgRole::Admin)
        );

        // Invitations: hashed token, pending, accepted.
        let invite_token = crate::auth::new_token();
        let invitation = Invitation {
            id: Uuid::new_v4(),
            org_id: org.id,
            kind: InvitationKind::OrgAdminSetup,
            project_id: None,
            email: "new-admin@test.local".into(),
            role: None,
            token_hash: crate::auth::hash_token(&invite_token),
            created_by: operator.id,
            expires_at: Utc::now() + Duration::days(7),
            accepted_at: None,
            accepted_user_id: None,
            revoked_at: None,
        };
        store
            .insert_invitation(&invitation, &invitation.token_hash)
            .await
            .expect("insert invitation");
        let pending = store
            .invitation_by_token_hash(&crate::auth::hash_token(&invite_token))
            .await
            .expect("lookup invitation")
            .expect("invitation row");
        assert!(pending.is_pending());
        assert_eq!(pending.kind, InvitationKind::OrgAdminSetup);
        store
            .mark_invitation_accepted(pending.id, operator.id)
            .await
            .expect("accept invitation");
        let accepted = store
            .invitation_by_id(pending.id)
            .await
            .expect("reload")
            .expect("row");
        assert!(!accepted.is_pending());
        assert_eq!(accepted.accepted_user_id, Some(operator.id));
        assert!(accepted.accepted_at.is_some());

        // Projects, members, keys, and API tokens.
        let dsn = store.next_dsn_id().await.expect("dsn id");
        assert_eq!(dsn, "1");
        let project = Project {
            id: Uuid::new_v4(),
            dsn_id: dsn,
            slug: "api".into(),
            name: "API".into(),
            org_id: org.id,
            retention_days: 30,
            model: Some(ModelConfig {
                endpoint: "https://model.internal".into(),
                api_key: Some("model-key".into()),
                model: "jev-latest".into(),
                confidence_threshold: 0.6,
            }),
            confidence_threshold: 0.6,
            deleted_at: None,
            deleted_by: None,
            created_at: Utc::now(),
        };
        store
            .insert_project(&project)
            .await
            .expect("insert project");
        assert_eq!(store.next_dsn_id().await.unwrap(), "2");
        assert_eq!(
            store.project_by_identifier("1").await.unwrap().unwrap().id,
            project.id
        );
        assert_eq!(
            store
                .project_by_identifier("api")
                .await
                .unwrap()
                .unwrap()
                .id,
            project.id
        );
        let loaded = store.project_by_id(project.id).await.unwrap().unwrap();
        assert_eq!(
            loaded
                .model
                .as_ref()
                .and_then(|model| model.api_key.as_deref()),
            Some("model-key"),
            "the replayable model key round-trips for server use only"
        );
        assert_eq!(store.projects_for_org(org.id).await.unwrap().len(), 1);

        // Members and roles.
        store
            .upsert_project_member(project.id, operator.id, Role::Owner)
            .await
            .expect("project member");
        assert_eq!(
            store.role_for(operator.id, project.id).await.unwrap(),
            Some(Role::Owner)
        );
        assert_eq!(store.owner_count(project.id).await.unwrap(), 1);
        let members = store.project_members(project.id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].0.id, operator.id);
        assert_eq!(members[0].1, Role::Owner);

        // Ingest keys: active lookup records the use, revoking ends it.
        let key = IngestKey {
            id: Uuid::new_v4(),
            project_id: project.id,
            label: "Default".into(),
            key_hash: crate::auth::hash_token("scout_key_one"),
            key_prefix: "scout_key_on".into(),
            created_at: Utc::now(),
            revoked: false,
        };
        store.insert_key(&key).await.expect("insert key");
        assert!(store
            .active_key_by_hash(&crate::auth::hash_token("scout_key_one"))
            .await
            .unwrap()
            .is_some());
        assert_eq!(store.keys_for_project(project.id).await.unwrap().len(), 1);
        assert_eq!(
            store.active_key_for(project.id).await.unwrap().unwrap().id,
            key.id
        );
        let used: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT last_used_at FROM ingest_keys WHERE id = $1")
                .bind(key.id)
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert!(used.is_some(), "a successful key lookup records use");
        assert!(store.revoke_key(project.id, key.id).await.unwrap());
        assert!(store.active_key_for(project.id).await.unwrap().is_none());
        assert_eq!(store.revoke_all_keys(project.id).await.unwrap(), 0);

        // API tokens: hash lookup, org listing, revocation.
        let api_token = ApiToken {
            id: Uuid::new_v4(),
            user_id: operator.id,
            label: "ci".into(),
            token_hash: crate::auth::hash_token("api_token_one"),
            token_prefix: "api_token_on".into(),
            created_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
        };
        store
            .insert_api_token(&api_token)
            .await
            .expect("insert api token");
        assert!(store
            .api_token_by_hash(&crate::auth::hash_token("api_token_one"))
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            store.api_tokens_for_user(operator.id).await.unwrap().len(),
            1
        );
        let for_org = store.api_tokens_for_org(org.id).await.unwrap();
        assert_eq!(for_org.len(), 1);
        assert_eq!(for_org[0].0, "ops@test.local");
        assert!(store
            .revoke_api_token(api_token.id, operator.id)
            .await
            .unwrap());
        assert!(!store
            .api_token_by_hash(&crate::auth::hash_token("api_token_one"))
            .await
            .unwrap()
            .unwrap()
            .is_active());

        // Soft delete hides the project from active lists but keeps the row.
        assert!(store
            .soft_delete_project(project.id, operator.id)
            .await
            .unwrap());
        assert_eq!(store.projects_for_org(org.id).await.unwrap().len(), 0);
        assert!(store
            .project_by_id(project.id)
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_some());
    }

    #[tokio::test]
    #[ignore = "requires SCOUT_TEST_DATABASE_URL"]
    async fn postgres_health_is_bounded_and_reports_durable() {
        let Some(store) = test_store().await else {
            return;
        };
        assert!(store.health().await);
        assert_eq!(store.kind(), "postgres");
        assert!(store.is_durable());
    }
}

// --- projects, members, keys, and API tokens (T024) ------------------------

const PROJECT_COLUMNS: &str = "id, dsn_id, slug, name, org_id, retention_days, model_endpoint, \
     model_name, model_api_key_ref, confidence_threshold, deleted_at, deleted_by, created_at";

fn project_from_row(row: &PgRow) -> Result<Project, sqlx::Error> {
    let confidence_threshold: f64 = row.try_get("confidence_threshold")?;
    let endpoint: Option<String> = row.try_get("model_endpoint")?;
    let model = match endpoint {
        Some(endpoint) => Some(ModelConfig {
            endpoint,
            api_key: row.try_get("model_api_key_ref")?,
            model: row
                .try_get::<Option<String>, _>("model_name")?
                .unwrap_or_default(),
            confidence_threshold,
        }),
        None => None,
    };
    Ok(Project {
        id: row.try_get("id")?,
        dsn_id: row.try_get("dsn_id")?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        org_id: row.try_get("org_id")?,
        retention_days: row.try_get::<i32, _>("retention_days")?.max(1) as u32,
        model,
        confidence_threshold,
        deleted_at: row.try_get("deleted_at")?,
        deleted_by: row.try_get("deleted_by")?,
        created_at: row.try_get("created_at")?,
    })
}

fn ingest_key_from_row(row: &PgRow) -> Result<IngestKey, sqlx::Error> {
    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at")?;
    Ok(IngestKey {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        label: row.try_get("label")?,
        key_hash: row.try_get("key_hash")?,
        key_prefix: row.try_get("key_prefix")?,
        created_at: row.try_get("created_at")?,
        revoked: revoked_at.is_some(),
    })
}

fn api_token_from_row(row: &PgRow) -> Result<ApiToken, sqlx::Error> {
    Ok(ApiToken {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        label: row.try_get("label")?,
        token_hash: row.try_get("token_hash")?,
        token_prefix: row.try_get("token_prefix")?,
        created_at: row.try_get("created_at")?,
        last_used_at: row.try_get("last_used_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}

impl PostgresStore {
    // --- projects ----------------------------------------------------------

    pub async fn project_by_id(&self, project_id: Uuid) -> Result<Option<Project>, String> {
        let row = sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects WHERE id = $1"
        ))
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref().map(project_from_row).transpose().map_err(db)
    }

    /// Resolve by DSN id or slug, for ingest and compatibility paths.
    pub async fn project_by_identifier(&self, identifier: &str) -> Result<Option<Project>, String> {
        let row = sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects WHERE dsn_id = $1 OR slug = $1 \
             ORDER BY (dsn_id = $1) DESC LIMIT 1"
        ))
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref().map(project_from_row).transpose().map_err(db)
    }

    /// Active projects in an organisation, newest first.
    pub async fn projects_for_org(&self, org_id: Uuid) -> Result<Vec<Project>, String> {
        let rows = sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects \
             WHERE org_id = $1 AND deleted_at IS NULL ORDER BY created_at"
        ))
        .bind(org_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(project_from_row)
            .collect::<Result<_, _>>()
            .map_err(db)
    }

    /// Next numeric DSN id. Slugs are never numeric in practice, and the filter
    /// keeps a hand-edited row from breaking allocation.
    pub async fn next_dsn_id(&self) -> Result<String, String> {
        let next: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(dsn_id::bigint), 0) + 1 FROM projects WHERE dsn_id ~ '^[0-9]+$'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(next.to_string())
    }

    pub async fn insert_project(&self, project: &Project) -> Result<(), String> {
        let (endpoint, model_name, api_key, threshold) = match &project.model {
            Some(model) => (
                Some(model.endpoint.clone()),
                Some(model.model.clone()),
                model.api_key.clone(),
                model.confidence_threshold,
            ),
            None => (None, None, None, project.confidence_threshold),
        };
        sqlx::query(
            "INSERT INTO projects (id, org_id, slug, name, dsn_id, retention_days, \
             model_endpoint, model_name, model_api_key_ref, confidence_threshold, deleted_at, \
             deleted_by, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(project.id)
        .bind(project.org_id)
        .bind(&project.slug)
        .bind(&project.name)
        .bind(&project.dsn_id)
        .bind(project.retention_days as i32)
        .bind(endpoint)
        .bind(model_name)
        .bind(api_key)
        .bind(threshold)
        .bind(project.deleted_at)
        .bind(project.deleted_by)
        .bind(project.created_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn update_project_settings(&self, project: &Project) -> Result<(), String> {
        let (endpoint, model_name, api_key, threshold) = match &project.model {
            Some(model) => (
                Some(model.endpoint.clone()),
                Some(model.model.clone()),
                model.api_key.clone(),
                model.confidence_threshold,
            ),
            None => (None, None, None, project.confidence_threshold),
        };
        sqlx::query(
            "UPDATE projects SET retention_days = $2, model_endpoint = $3, model_name = $4, \
             model_api_key_ref = $5, confidence_threshold = $6 WHERE id = $1",
        )
        .bind(project.id)
        .bind(project.retention_days as i32)
        .bind(endpoint)
        .bind(model_name)
        .bind(api_key)
        .bind(threshold)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Soft delete: rows, objects, and logs stay and remain subject to retention.
    pub async fn soft_delete_project(
        &self,
        project_id: Uuid,
        deleted_by: Uuid,
    ) -> Result<bool, String> {
        let result = sqlx::query(
            "UPDATE projects SET deleted_at = now(), deleted_by = $2 \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(project_id)
        .bind(deleted_by)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    // --- project members ---------------------------------------------------

    pub async fn role_for(&self, user_id: Uuid, project_id: Uuid) -> Result<Option<Role>, String> {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2",
        )
        .bind(project_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(role.as_deref().and_then(|role| parse_role(Some(role))))
    }

    pub async fn upsert_project_member(
        &self,
        project_id: Uuid,
        user_id: Uuid,
        role: Role,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, $3) \
             ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role",
        )
        .bind(project_id)
        .bind(user_id)
        .bind(role.to_string())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn remove_project_member(
        &self,
        project_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, String> {
        let result =
            sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
                .bind(project_id)
                .bind(user_id)
                .execute(&self.pool)
                .await
                .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    /// Members with their accounts, strongest role first.
    pub async fn project_members(&self, project_id: Uuid) -> Result<Vec<(User, Role)>, String> {
        let rows = sqlx::query(&format!(
            "SELECT u.id, u.email::text AS email, u.name, u.password_hash, u.is_operator, \
             u.disabled_at, u.locked_at, u.failed_sign_in_count, m.role \
             FROM project_members m JOIN users u ON u.id = m.user_id \
             WHERE m.project_id = $1 \
             ORDER BY CASE m.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 \
                      WHEN 'triager' THEN 2 ELSE 3 END, u.name"
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|row| {
                let role: String = row.try_get("role")?;
                Ok((
                    user_from_row(row)?,
                    parse_role(Some(&role)).unwrap_or(Role::Viewer),
                ))
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(db)
    }

    pub async fn owner_count(&self, project_id: Uuid) -> Result<i64, String> {
        sqlx::query_scalar(
            "SELECT count(*) FROM project_members WHERE project_id = $1 AND role = 'owner'",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)
    }

    // --- ingest keys -------------------------------------------------------

    pub async fn insert_key(&self, key: &IngestKey) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO ingest_keys (id, project_id, label, key_hash, key_prefix, created_at, \
             revoked_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(key.id)
        .bind(key.project_id)
        .bind(&key.label)
        .bind(&key.key_hash)
        .bind(&key.key_prefix)
        .bind(key.created_at)
        .bind(if key.revoked { Some(Utc::now()) } else { None })
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn keys_for_project(&self, project_id: Uuid) -> Result<Vec<IngestKey>, String> {
        let rows = sqlx::query(
            "SELECT id, project_id, label, key_hash, key_prefix, created_at, revoked_at \
             FROM ingest_keys WHERE project_id = $1 ORDER BY created_at",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(ingest_key_from_row)
            .collect::<Result<_, _>>()
            .map_err(db)
    }

    /// The oldest active key, which settings and test events use.
    pub async fn active_key_for(&self, project_id: Uuid) -> Result<Option<IngestKey>, String> {
        let row = sqlx::query(
            "SELECT id, project_id, label, key_hash, key_prefix, created_at, revoked_at \
             FROM ingest_keys WHERE project_id = $1 AND revoked_at IS NULL \
             ORDER BY created_at LIMIT 1",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref()
            .map(ingest_key_from_row)
            .transpose()
            .map_err(db)
    }

    /// Resolve an ingest credential by hash, recording the last use.
    pub async fn active_key_by_hash(&self, key_hash: &str) -> Result<Option<IngestKey>, String> {
        let row = sqlx::query(
            "SELECT id, project_id, label, key_hash, key_prefix, created_at, revoked_at \
             FROM ingest_keys WHERE key_hash = $1 AND revoked_at IS NULL",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let key = ingest_key_from_row(&row).map_err(db)?;
        sqlx::query("UPDATE ingest_keys SET last_used_at = now() WHERE id = $1")
            .bind(key.id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(Some(key))
    }

    pub async fn revoke_key(&self, project_id: Uuid, key_id: Uuid) -> Result<bool, String> {
        let result = sqlx::query(
            "UPDATE ingest_keys SET revoked_at = now() \
             WHERE id = $1 AND project_id = $2 AND revoked_at IS NULL",
        )
        .bind(key_id)
        .bind(project_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn revoke_all_keys(&self, project_id: Uuid) -> Result<u64, String> {
        let result = sqlx::query(
            "UPDATE ingest_keys SET revoked_at = now() \
             WHERE project_id = $1 AND revoked_at IS NULL",
        )
        .bind(project_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(result.rows_affected())
    }

    // --- API tokens --------------------------------------------------------

    pub async fn insert_api_token(&self, token: &ApiToken) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO api_tokens (id, user_id, label, token_hash, token_prefix, created_at, \
             last_used_at, revoked_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(token.id)
        .bind(token.user_id)
        .bind(&token.label)
        .bind(&token.token_hash)
        .bind(&token.token_prefix)
        .bind(token.created_at)
        .bind(token.last_used_at)
        .bind(token.revoked_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Resolve a compatibility token by hash, recording the last use.
    pub async fn api_token_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>, String> {
        let row = sqlx::query(
            "SELECT id, user_id, label, token_hash, token_prefix, created_at, last_used_at, \
             revoked_at FROM api_tokens WHERE token_hash = $1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let token = api_token_from_row(&row).map_err(db)?;
        if token.is_active() {
            sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1")
                .bind(token.id)
                .execute(&self.pool)
                .await
                .map_err(db)?;
        }
        Ok(Some(token))
    }

    pub async fn api_tokens_for_user(&self, user_id: Uuid) -> Result<Vec<ApiToken>, String> {
        let rows = sqlx::query(
            "SELECT id, user_id, label, token_hash, token_prefix, created_at, last_used_at, \
             revoked_at FROM api_tokens WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(api_token_from_row)
            .collect::<Result<_, _>>()
            .map_err(db)
    }

    /// Tokens owned by anyone who belongs to the organisation, with the owner's
    /// email. Metadata only.
    pub async fn api_tokens_for_org(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<(String, ApiToken)>, String> {
        let rows = sqlx::query(
            "SELECT t.id, t.user_id, t.label, t.token_hash, t.token_prefix, t.created_at, \
             t.last_used_at, t.revoked_at, u.email::text AS email \
             FROM api_tokens t JOIN users u ON u.id = t.user_id \
             WHERE t.user_id IN ( \
                 SELECT user_id FROM org_members WHERE org_id = $1 \
                 UNION \
                 SELECT pm.user_id FROM project_members pm \
                 JOIN projects p ON p.id = pm.project_id WHERE p.org_id = $1 \
             ) ORDER BY t.created_at DESC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|row| {
                let email: String = row.try_get("email")?;
                Ok((email, api_token_from_row(row)?))
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(db)
    }

    pub async fn revoke_api_token(&self, token_id: Uuid, user_id: Uuid) -> Result<bool, String> {
        let result = sqlx::query(
            "UPDATE api_tokens SET revoked_at = now() \
             WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
        )
        .bind(token_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }
}
