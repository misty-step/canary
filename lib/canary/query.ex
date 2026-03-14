defmodule Canary.Query do
  @moduledoc """
  Query API logic. Reads from error_groups and target_state.
  All responses include deterministic summary strings.
  """

  alias Canary.ReadRepo
  alias Canary.Schemas.{Error, ErrorGroup, Target, TargetCheck, TargetState}
  import Ecto.Query

  @max_groups 50
  @allowed_windows ~w(1h 6h 24h 7d 30d)

  def errors_by_service(service, window, cursor \\ nil) do
    with {:ok, cutoff} <- window_to_cutoff(window) do
      query =
        from(g in ErrorGroup,
          where: g.service == ^service and g.last_seen_at >= ^cutoff,
          order_by: [desc: g.total_count],
          limit: ^@max_groups
        )

      query = apply_cursor(query, cursor)
      groups = ReadRepo.all(query)

      total = Enum.reduce(groups, 0, &(&1.total_count + &2))

      summary =
        Canary.Summary.error_query(%{
          total: total,
          service: service,
          window: window,
          groups: groups
        })

      next_cursor =
        if length(groups) == @max_groups do
          groups |> List.last() |> Map.get(:group_hash) |> Base.encode64()
        end

      {:ok,
       %{
         summary: summary,
         service: service,
         window: window,
         total_errors: total,
         groups:
           Enum.map(groups, fn g ->
             %{
               group_hash: g.group_hash,
               error_class: g.error_class,
               count: g.total_count,
               first_seen: g.first_seen_at,
               last_seen: g.last_seen_at,
               sample_message: g.message_template,
               severity: g.severity,
               status: g.status
             }
           end),
         cursor: next_cursor
       }}
    end
  end

  def errors_by_class(window) do
    with {:ok, cutoff} <- window_to_cutoff(window) do
      groups =
        from(g in ErrorGroup,
          where: g.last_seen_at >= ^cutoff,
          select: %{
            error_class: g.error_class,
            total_count: sum(g.total_count),
            service_count: count(fragment("DISTINCT ?", g.service))
          },
          group_by: g.error_class,
          order_by: [desc: sum(g.total_count)],
          limit: 50
        )
        |> ReadRepo.all()

      {:ok, %{window: window, groups: groups}}
    end
  end

  def error_detail(error_id) do
    case ReadRepo.get(Error, error_id) do
      nil ->
        {:error, :not_found}

      error ->
        group = ReadRepo.get(ErrorGroup, error.group_hash)

        summary =
          Canary.Summary.error_detail(%{
            error_class: error.error_class,
            service: error.service,
            count: (group && group.total_count) || 1,
            first_seen: (group && group.first_seen_at) || error.created_at,
            last_seen: (group && group.last_seen_at) || error.created_at
          })

        {:ok,
         %{
           summary: summary,
           id: error.id,
           service: error.service,
           error_class: error.error_class,
           message: error.message,
           message_template: error.message_template,
           stack_trace: error.stack_trace,
           context: safe_decode_json(error.context),
           severity: error.severity,
           environment: error.environment,
           group_hash: error.group_hash,
           created_at: error.created_at,
           group: group && %{
             total_count: group.total_count,
             first_seen_at: group.first_seen_at,
             last_seen_at: group.last_seen_at,
             status: group.status
           }
         }}
    end
  end

  def health_status do
    targets = from(t in Target, order_by: t.name) |> ReadRepo.all()

    enriched =
      Enum.map(targets, fn target ->
        state = ReadRepo.get(TargetState, target.id)

        recent_checks =
          from(c in TargetCheck,
            where: c.target_id == ^target.id,
            order_by: [desc: c.checked_at],
            limit: 5
          )
          |> ReadRepo.all()

        %{
          id: target.id,
          name: target.name,
          url: target.url,
          state: (state && state.state) || "unknown",
          consecutive_failures: (state && state.consecutive_failures) || 0,
          last_checked_at: state && state.last_checked_at,
          last_success_at: state && state.last_success_at,
          latency_ms: recent_checks |> List.first() |> then(&(&1 && &1.latency_ms)),
          tls_expires_at: recent_checks |> Enum.find(&(&1 && &1.tls_expires_at)) |> then(&(&1 && &1.tls_expires_at)),
          recent_checks:
            Enum.map(recent_checks, fn c ->
              %{
                checked_at: c.checked_at,
                result: c.result,
                status_code: c.status_code,
                latency_ms: c.latency_ms
              }
            end)
        }
      end)

    summary = Canary.Summary.health_status(%{targets: enriched})

    %{summary: summary, targets: enriched}
  end

  def target_checks(target_id, window) do
    with {:ok, cutoff} <- window_to_cutoff(window) do
      checks =
        from(c in TargetCheck,
          where: c.target_id == ^target_id and c.checked_at >= ^cutoff,
          order_by: [desc: c.checked_at],
          limit: 500
        )
        |> ReadRepo.all()

      {:ok, checks}
    end
  end

  # --- Helpers ---

  defp window_to_cutoff(window) when window in @allowed_windows do
    seconds =
      case window do
        "1h" -> 3_600
        "6h" -> 21_600
        "24h" -> 86_400
        "7d" -> 604_800
        "30d" -> 2_592_000
      end

    cutoff =
      DateTime.utc_now()
      |> DateTime.add(-seconds, :second)
      |> DateTime.to_iso8601()

    {:ok, cutoff}
  end

  defp window_to_cutoff(_), do: {:error, :invalid_window}

  defp apply_cursor(query, nil), do: query

  defp apply_cursor(query, cursor) do
    case Base.decode64(cursor) do
      {:ok, after_hash} ->
        from(g in query, where: g.group_hash > ^after_hash)

      _ ->
        query
    end
  end

  defp safe_decode_json(nil), do: nil
  defp safe_decode_json(json) when is_binary(json) do
    case Jason.decode(json) do
      {:ok, decoded} -> decoded
      _ -> json
    end
  end
  defp safe_decode_json(other), do: other
end
