//! Shared tenant/project owner-scope helpers for query and SLI read models.
//!
//! Both `query.rs` and `service_sli.rs` need to scope SQL by tenant and
//! project. This module owns the one canonical clause builder and parameter
//! list so cross-tenant isolation logic cannot drift between the two copies.

/// Build the `AND alias.tenant_id = ?N AND alias.project_id = ?N+1` SQL
/// fragment. Returns an empty string when `owner` is `None` so callers can
/// always interpolate `{owner_clause}` into a single SQL template.
pub(crate) fn owner_clause(
    alias: &str,
    first_parameter: usize,
    owner: Option<(&str, &str)>,
) -> String {
    owner
        .map(|_| {
            format!(
                " AND {alias}.tenant_id = ?{first_parameter} AND {alias}.project_id = ?{}",
                first_parameter + 1
            )
        })
        .unwrap_or_default()
}

/// Collect tenant and project parameter values for a scoped query. Returns
/// an empty vector when `owner` is `None`.
pub(crate) fn owner_params<'a>(owner: Option<(&'a str, &'a str)>) -> Vec<&'a str> {
    owner
        .map(|(tenant_id, project_id)| vec![tenant_id, project_id])
        .unwrap_or_default()
}

/// Prepend `cutoff` to the owner params, producing the parameter list for a
/// windowed scoped query: `[cutoff, tenant_id, project_id]` or `[cutoff]`.
pub(crate) fn window_params<'a>(
    cutoff: &'a str,
    owner: Option<(&'a str, &'a str)>,
) -> Vec<&'a str> {
    let mut values = vec![cutoff];
    if let Some((tenant_id, project_id)) = owner {
        values.push(tenant_id);
        values.push(project_id);
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_clause_with_owner_produces_scoped_fragment() {
        let clause = owner_clause("error_groups", 2, Some(("tenant-a", "project-b")));
        assert!(clause.contains("error_groups.tenant_id = ?2"));
        assert!(clause.contains("error_groups.project_id = ?3"));
    }

    #[test]
    fn owner_clause_without_owner_returns_empty_string() {
        let clause = owner_clause("incidents", 1, None);
        assert!(clause.is_empty());
    }

    #[test]
    fn owner_clause_preserves_alias_and_first_parameter() {
        let clause = owner_clause("g", 5, Some(("t", "p")));
        assert!(clause.contains("g.tenant_id = ?5"));
        assert!(clause.contains("g.project_id = ?6"));
    }

    #[test]
    fn owner_params_with_owner_returns_both_values() {
        let params = owner_params(Some(("tenant-a", "project-b")));
        assert_eq!(params, vec!["tenant-a", "project-b"]);
    }

    #[test]
    fn owner_params_without_owner_returns_empty_vec() {
        let params = owner_params(None);
        assert!(params.is_empty());
    }

    #[test]
    fn window_params_with_owner_returns_cutoff_tenant_project() {
        let params = window_params("2026-07-01T00:00:00Z", Some(("tenant-a", "project-b")));
        assert_eq!(
            params,
            vec!["2026-07-01T00:00:00Z", "tenant-a", "project-b"]
        );
    }

    #[test]
    fn window_params_without_owner_returns_cutoff_only() {
        let params = window_params("2026-07-01T00:00:00Z", None);
        assert_eq!(params, vec!["2026-07-01T00:00:00Z"]);
    }
}
