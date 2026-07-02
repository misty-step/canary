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
