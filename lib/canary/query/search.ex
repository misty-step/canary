defmodule Canary.Query.Search do
  @moduledoc false
  require Logger

  @default_limit 20
  @bm25_weights "1.0, 2.0, 5.0, 1.0"

  def search(query, opts \\ [])

  def search(query, opts) when is_binary(query) do
    trimmed = String.trim(query)

    if trimmed == "" do
      {:ok, []}
    else
      run_search(trimmed, opts)
    end
  end

  def search(_query, _opts), do: {:error, :invalid_query}

  defp run_search(query, opts) do
    service = Keyword.get(opts, :service)
    cutoff = Keyword.get(opts, :cutoff)
    limit = Keyword.get(opts, :limit, @default_limit)

    {sql, params} = search_sql(quoted_query(query), service, cutoff, limit)

    case Canary.Repos.read_repo().query(sql, params) do
      {:ok, result} ->
        {:ok, Enum.map(result.rows, &format_result/1)}

      {:error, reason} ->
        Logger.warning("Search query failed: #{inspect(reason)}")
        {:error, :search_failed}
    end
  end

  defp search_sql(query, service, cutoff, limit) do
    {clauses, params} =
      {["errors_fts MATCH ?"], [query]}
      |> maybe_add_clause(service, "e.service = ?")
      |> maybe_add_clause(cutoff, "e.created_at >= ?")

    {"""
     SELECT e.id, e.service, e.error_class, e.message, e.group_hash, e.created_at,
            -bm25(errors_fts, #{@bm25_weights}) AS score
     FROM errors_fts
     JOIN errors AS e ON e.rowid = errors_fts.rowid
     WHERE #{Enum.join(clauses, "\n       AND ")}
     ORDER BY score DESC, e.created_at DESC
     LIMIT ?
     """, params ++ [limit]}
  end

  defp quoted_query(query) do
    escaped = String.replace(query, "\"", "\"\"")
    ~s("#{escaped}")
  end

  defp maybe_add_clause({clauses, params}, nil, _clause), do: {clauses, params}

  defp maybe_add_clause({clauses, params}, value, clause) do
    {clauses ++ [clause], params ++ [value]}
  end

  # format_result/1 must stay in the same column order as search_sql/4.
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
