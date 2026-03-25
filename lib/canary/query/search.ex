defmodule Canary.Query.Search do
  @moduledoc false

  @default_limit 20

  def search(query, opts \\ [])

  def search(query, opts) when is_binary(query) do
    trimmed = String.trim(query)

    if trimmed == "" do
      {:ok, []}
    else
      run_search(trimmed, opts)
    end
  end

  defp run_search(query, opts) do
    service = Keyword.get(opts, :service)
    limit = Keyword.get(opts, :limit, @default_limit)

    {sql, params} = search_sql(quoted_query(query), service, limit)
    result = Canary.Repos.read_repo().query!(sql, params)

    {:ok, Enum.map(result.rows, &format_result/1)}
  end

  defp search_sql(query, nil, limit) do
    {"""
     SELECT e.id, e.service, e.error_class, e.message, e.group_hash, e.created_at,
            -bm25(errors_fts, 1.0, 2.0, 5.0, 1.0) AS score
     FROM errors_fts
     JOIN errors AS e ON e.rowid = errors_fts.rowid
     WHERE errors_fts MATCH ?
     ORDER BY score DESC, e.created_at DESC
     LIMIT ?
     """, [query, limit]}
  end

  defp search_sql(query, service, limit) do
    {"""
     SELECT e.id, e.service, e.error_class, e.message, e.group_hash, e.created_at,
            -bm25(errors_fts, 1.0, 2.0, 5.0, 1.0) AS score
     FROM errors_fts
     JOIN errors AS e ON e.rowid = errors_fts.rowid
     WHERE errors_fts MATCH ?
       AND e.service = ?
     ORDER BY score DESC, e.created_at DESC
     LIMIT ?
     """, [query, service, limit]}
  end

  defp quoted_query(query) do
    escaped = String.replace(query, "\"", "\"\"")
    ~s("#{escaped}")
  end

  defp format_result([id, service, error_class, message, group_hash, created_at, score]) do
    %{
      id: id,
      service: service,
      error_class: error_class,
      message: message,
      group_hash: group_hash,
      created_at: created_at,
      score: score
    }
  end
end
