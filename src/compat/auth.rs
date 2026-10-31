//! Bearer-token authentication and live scope derivation for `/api/0/`.
//!
//! Tokens carry no scope list: every request resolves the owner's current project
//! and organisation roles and derives the Sentry scope set from them.

use crate::compat::error::CompatError;
use crate::models::{OrgRole, Organisation, Project, Role};
use crate::state::AppState;
use actix_web::HttpRequest;
use std::collections::HashSet;
use std::fmt;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    EventRead,
    EventWrite,
    EventAdmin,
    ProjectRead,
    ProjectWrite,
    ProjectAdmin,
    ProjectReleases,
    OrgRead,
    OrgAdmin,
    OrgIntegrations,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::EventRead => "event:read",
            Scope::EventWrite => "event:write",
            Scope::EventAdmin => "event:admin",
            Scope::ProjectRead => "project:read",
            Scope::ProjectWrite => "project:write",
            Scope::ProjectAdmin => "project:admin",
            Scope::ProjectReleases => "project:releases",
            Scope::OrgRead => "org:read",
            Scope::OrgAdmin => "org:admin",
            Scope::OrgIntegrations => "org:integrations",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The authenticated token owner. Scopes are resolved against the addressed
/// project/org by [`authorize_project`] and [`authorize_org`].
#[derive(Clone, Debug)]
pub struct CompatAuth {
    pub user_id: Uuid,
    pub email: String,
}

impl CompatAuth {
    pub fn require(&self, scopes: &HashSet<Scope>, scope: Scope) -> Result<(), CompatError> {
        if scopes.contains(&scope) {
            Ok(())
        } else {
            Err(CompatError::Forbidden(format!(
                "missing required scope {scope}"
            )))
        }
    }
}

/// Sentry scopes granted by a project role plus an optional organisation role.
pub fn scopes_for(role: Role, org_role: Option<OrgRole>) -> HashSet<Scope> {
    let mut scopes = HashSet::new();
    scopes.insert(Scope::EventRead);
    scopes.insert(Scope::ProjectRead);
    if role >= Role::Triager {
        scopes.insert(Scope::EventWrite);
    }
    if role >= Role::Admin {
        scopes.insert(Scope::ProjectWrite);
        scopes.insert(Scope::ProjectAdmin);
        scopes.insert(Scope::EventAdmin);
        scopes.insert(Scope::ProjectReleases);
    }
    match org_role {
        Some(OrgRole::Admin) => {
            scopes.insert(Scope::OrgRead);
            scopes.insert(Scope::OrgAdmin);
            scopes.insert(Scope::OrgIntegrations);
        }
        Some(OrgRole::Member) => {
            scopes.insert(Scope::OrgRead);
        }
        None => {}
    }
    scopes
}

fn bearer(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Resolve the presented bearer token to its owner. Every failure mode maps to
/// the same `401 Invalid token` body.
pub async fn authenticate(req: &HttpRequest, state: &AppState) -> Result<CompatAuth, CompatError> {
    let plaintext = bearer(req).ok_or(CompatError::Unauthorized)?;
    if !plaintext.starts_with("scout_pat_") {
        return Err(CompatError::Unauthorized);
    }
    let (user_id, email, token_id) = {
        let store = state.store.read().await;
        let token = store
            .active_api_token(&plaintext)
            .ok_or(CompatError::Unauthorized)?;
        let user = store
            .users
            .get(&token.user_id)
            .ok_or(CompatError::Unauthorized)?;
        if user.disabled_at.is_some() {
            return Err(CompatError::Unauthorized);
        }
        (user.id, user.email.clone(), token.id)
    };
    {
        let mut store = state.store.write().await;
        store.touch_api_token(token_id);
    }
    Ok(CompatAuth { user_id, email })
}

/// Resolve an organisation by slug or UUID.
pub fn resolve_org<'a>(
    store: &'a crate::store::Store,
    reference: &str,
) -> Option<&'a Organisation> {
    if let Ok(id) = Uuid::parse_str(reference) {
        if let Some(org) = store.orgs.get(&id) {
            return Some(org);
        }
    }
    store
        .orgs
        .values()
        .find(|org| org.slug.eq_ignore_ascii_case(reference))
}

/// Resolve a project inside an organisation by slug, UUID, or DSN id. Soft-deleted
/// projects behave as unknown.
pub fn resolve_project<'a>(
    store: &'a crate::store::Store,
    org: &Organisation,
    reference: &str,
) -> Option<&'a Project> {
    let project = if let Ok(id) = Uuid::parse_str(reference) {
        store.projects.get(&id)
    } else if let Some(id) = store.projects_by_dsn.get(reference) {
        store.projects.get(id)
    } else {
        store.projects.values().find(|project| {
            project.org_id == org.id && project.slug.eq_ignore_ascii_case(reference)
        })
    }?;
    if project.org_id == org.id && project.deleted_at.is_none() {
        Some(project)
    } else {
        None
    }
}

/// Authenticate and resolve the addressed project. Unknown org/project and a
/// missing project role both produce `404`, so existence never leaks.
pub async fn authorize_project(
    state: &AppState,
    auth: &CompatAuth,
    org_ref: &str,
    project_ref: &str,
    scope: Scope,
) -> Result<(Organisation, Project, HashSet<Scope>), CompatError> {
    let (org, project, role, org_role) = {
        let store = state.store.read().await;
        let org = resolve_org(&store, org_ref)
            .ok_or_else(|| CompatError::NotFound("organisation not found".into()))?
            .clone();
        let project = resolve_project(&store, &org, project_ref)
            .ok_or_else(|| CompatError::NotFound("project not found".into()))?
            .clone();
        let role = store
            .role_for(auth.user_id, project.id)
            .ok_or_else(|| CompatError::NotFound("project not found".into()))?;
        let org_role = store.org_role_for(auth.user_id, org.id);
        (org, project, role, org_role)
    };
    let scopes = scopes_for(role, org_role);
    auth.require(&scopes, scope)?;
    Ok((org, project, scopes))
}

/// Authenticate at organisation scope (app installations). A user with no role in
/// the organisation sees `404`.
pub async fn authorize_org(
    state: &AppState,
    auth: &CompatAuth,
    org_ref: &str,
    scope: Scope,
) -> Result<Organisation, CompatError> {
    let (org, org_role) = {
        let store = state.store.read().await;
        let org = resolve_org(&store, org_ref)
            .ok_or_else(|| CompatError::NotFound("organisation not found".into()))?
            .clone();
        let org_role = store.org_role_for(auth.user_id, org.id);
        (org, org_role)
    };
    let org_role =
        org_role.ok_or_else(|| CompatError::NotFound("organisation not found".into()))?;
    let scopes = scopes_for(Role::Viewer, Some(org_role));
    auth.require(&scopes, scope)?;
    Ok(org)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_gets_read_only() {
        let scopes = scopes_for(Role::Viewer, None);
        assert!(scopes.contains(&Scope::EventRead));
        assert!(scopes.contains(&Scope::ProjectRead));
        assert!(!scopes.contains(&Scope::EventWrite));
        assert!(!scopes.contains(&Scope::ProjectAdmin));
    }

    #[test]
    fn triager_can_write_events_but_not_manage() {
        let scopes = scopes_for(Role::Triager, None);
        assert!(scopes.contains(&Scope::EventWrite));
        assert!(!scopes.contains(&Scope::ProjectAdmin));
        assert!(!scopes.contains(&Scope::ProjectReleases));
    }

    #[test]
    fn admin_gets_project_management() {
        let scopes = scopes_for(Role::Admin, None);
        for scope in [
            Scope::EventAdmin,
            Scope::ProjectWrite,
            Scope::ProjectAdmin,
            Scope::ProjectReleases,
        ] {
            assert!(scopes.contains(&scope), "missing {scope}");
        }
    }

    #[test]
    fn org_roles_add_org_scopes() {
        let member = scopes_for(Role::Viewer, Some(OrgRole::Member));
        assert!(member.contains(&Scope::OrgRead));
        assert!(!member.contains(&Scope::OrgAdmin));

        let admin = scopes_for(Role::Viewer, Some(OrgRole::Admin));
        assert!(admin.contains(&Scope::OrgAdmin));
        assert!(admin.contains(&Scope::OrgIntegrations));
    }
}
