defmodule Canary.Timeline do
  @moduledoc """
  Canonical append-only event log for service-level observability facts.
  """

  import Ecto.Query

  alias Canary.{ID, Repo}
  alias Canary.Schemas.{Error, Incident, ServiceEvent, Target, TargetState}

  @default_limit 50
  @max_limit 200

  @spec record_error(String.t(), Error.t(), String.t()) :: {:ok, map()} | {:error, term()}
  def record_error(event, %Error{} = error, group_hash) do
    payload = %{
      "event" => event,
      "error" => %{
        "id" => error.id,
        "service" => error.service,
        "error_class" => error.error_class,
        "message" => error.message,
        "severity" => error.severity,
        "group_hash" => group_hash
      },
      "timestamp" => error.created_at
    }

    summary =
      case event do
        "error.new_class" -> "#{error.service}: new #{error.error_class}"
        "error.regression" -> "#{error.service}: #{error.error_class} regressed"
        _ -> "#{error.service}: #{event}"
      end

    attrs = %{
      service: error.service,
      event: event,
      entity_type: "error_group",
      entity_ref: group_hash,
      severity: error.severity,
      summary: summary,
      payload: payload,
      created_at: error.created_at
    }

    with {:ok, _service_event} <- insert_event(attrs) do
      {:ok, payload}
    end
  end

  @spec record_error!(String.t(), Error.t(), String.t()) :: map()
  def record_error!(event, %Error{} = error, group_hash) do
    case record_error(event, error, group_hash) do
      {:ok, payload} -> payload
      {:error, reason} -> raise "failed to record error event #{event}: #{inspect(reason)}"
    end
  end

  @spec record_health_transition!(
          String.t(),
          Target.t(),
          atom(),
          atom(),
          TargetState.t() | nil,
          integer(),
          String.t()
        ) :: map()
  def record_health_transition!(event, %Target{} = target, old_state, new_state, state, seq, now) do
    payload = %{
      "event" => event,
      "target" => %{
        "name" => target.name,
        "service" => Target.service_name(target),
        "url" => target.url
      },
      "state" => to_string(new_state),
      "previous_state" => to_string(old_state),
      "consecutive_failures" => (state && state.consecutive_failures) || 0,
      "last_success_at" => state && state.last_success_at,
      "sequence" => seq,
      "timestamp" => now
    }

    insert_event!(%{
      service: Target.service_name(target),
      event: event,
      entity_type: "target",
      entity_ref: target.id,
      severity: health_severity(new_state),
      summary: "#{Target.service_name(target)}: #{target.name} #{to_string(new_state)}",
      payload: payload,
      created_at: now
    })

    payload
  end

  @spec record_incident!(String.t(), Incident.t(), String.t()) :: map()
  def record_incident!(event, %Incident{} = incident, now) do
    payload = %{
      "event" => event,
      "incident" => incident_payload(incident),
      "timestamp" => now
    }

    summary =
      case event do
        "incident.opened" -> "#{incident.service}: incident opened"
        "incident.updated" -> "#{incident.service}: incident updated"
        "incident.resolved" -> "#{incident.service}: incident resolved"
        _ -> "#{incident.service}: #{event}"
      end

    insert_event!(%{
      service: incident.service,
      event: event,
      entity_type: "incident",
      entity_ref: incident.id,
      severity: incident.severity,
      summary: summary,
      payload: payload,
      created_at: now
    })

    payload
  end

  @spec record_tls_expiring!(Target.t(), String.t(), integer(), String.t()) :: map()
  def record_tls_expiring!(%Target{} = target, expiry_str, days_until, now) do
    payload = %{
      "event" => "health_check.tls_expiring",
      "target" => %{
        "name" => target.name,
        "service" => Target.service_name(target),
        "url" => target.url
      },
      "tls_expires_at" => expiry_str,
      "days_until_expiry" => days_until,
      "timestamp" => now
    }

    insert_event!(%{
      service: Target.service_name(target),
      event: "health_check.tls_expiring",
      entity_type: "target",
      entity_ref: target.id,
      severity: "warning",
      summary: "#{Target.service_name(target)}: TLS expires in #{days_until} day(s)",
      payload: payload,
      created_at: now
    })

    payload
  end

  @spec list(keyword()) ::
          {:ok,
           %{
             summary: String.t(),
             returned_count: non_neg_integer(),
             window: String.t(),
             service: String.t() | nil,
             events: list(),
             cursor: String.t() | nil
           }}
          | {:error, :invalid_cursor | :invalid_limit | :invalid_window}
  def list(opts \\ []) do
    window = Keyword.get(opts, :window, "24h")
    service = Keyword.get(opts, :service)

    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window),
         {:ok, limit} <- parse_limit(Keyword.get(opts, :limit)),
         {:ok, cursor} <- decode_cursor(Keyword.get(opts, :cursor)) do
      query =
        from(e in ServiceEvent,
          where: e.created_at >= ^cutoff,
          order_by: [desc: e.created_at, desc: e.id],
          limit: ^(limit + 1)
        )
        |> maybe_filter_service(service)
        |> maybe_apply_cursor(cursor)

      rows = Canary.Repos.read_repo().all(query)
      {events, next_cursor} = paginate(rows, limit)

      {:ok,
       %{
         summary: summary_for(events, service, window),
         returned_count: length(events),
         window: window,
         service: service,
         events: Enum.map(events, &format_event/1),
         cursor: next_cursor
       }}
    end
  end

  defp insert_event(attrs) do
    payload = Jason.encode!(attrs.payload)

    attrs = %{
      service: attrs.service,
      event: attrs.event,
      entity_type: attrs.entity_type,
      entity_ref: attrs.entity_ref,
      severity: attrs.severity,
      summary: attrs.summary,
      payload: payload,
      created_at: attrs.created_at
    }

    ID.event_id()
    |> ServiceEvent.with_id(attrs)
    |> Repo.insert()
  end

  defp insert_event!(attrs) do
    case insert_event(attrs) do
      {:ok, service_event} -> service_event
      {:error, reason} -> raise "failed to insert service event: #{inspect(reason)}"
    end
  end

  defp health_severity(:down), do: "error"
  defp health_severity(:degraded), do: "warning"
  defp health_severity(:up), do: "info"
  defp health_severity(_), do: "info"

  defp incident_payload(incident) do
    %{
      "id" => incident.id,
      "service" => incident.service,
      "state" => incident.state,
      "severity" => incident.severity,
      "title" => incident.title,
      "opened_at" => incident.opened_at,
      "resolved_at" => incident.resolved_at,
      "signals" =>
        Enum.map(incident.signals, fn signal ->
          %{
            "signal_type" => signal.signal_type,
            "signal_ref" => signal.signal_ref,
            "attached_at" => signal.attached_at,
            "resolved_at" => signal.resolved_at
          }
        end)
    }
  end

  defp parse_limit(nil), do: {:ok, @default_limit}

  defp parse_limit(limit) when is_integer(limit) and limit > 0 and limit <= @max_limit,
    do: {:ok, limit}

  defp parse_limit(limit) when is_binary(limit) do
    case Integer.parse(limit) do
      {value, ""} when value > 0 and value <= @max_limit -> {:ok, value}
      _ -> {:error, :invalid_limit}
    end
  end

  defp parse_limit(_), do: {:error, :invalid_limit}

  defp decode_cursor(nil), do: {:ok, nil}
  defp decode_cursor(""), do: {:ok, nil}

  defp decode_cursor(cursor) when is_binary(cursor) do
    with {:ok, decoded} <- Base.url_decode64(cursor, padding: false),
         {:ok, %{"created_at" => created_at, "id" => id}} <- Jason.decode(decoded),
         true <- is_binary(created_at) and is_binary(id),
         {:ok, _dt, _offset} <- DateTime.from_iso8601(created_at) do
      {:ok, %{created_at: created_at, id: id}}
    else
      _ -> {:error, :invalid_cursor}
    end
  end

  defp decode_cursor(_), do: {:error, :invalid_cursor}

  defp maybe_filter_service(query, nil), do: query
  defp maybe_filter_service(query, ""), do: query
  defp maybe_filter_service(query, service), do: from(e in query, where: e.service == ^service)

  defp maybe_apply_cursor(query, nil), do: query

  defp maybe_apply_cursor(query, %{created_at: created_at, id: id}) do
    from(e in query,
      where: e.created_at < ^created_at or (e.created_at == ^created_at and e.id < ^id)
    )
  end

  defp paginate(rows, limit) do
    {page, rest} = Enum.split(rows, limit)

    next_cursor =
      case {page, rest} do
        {[], _} ->
          nil

        {_, []} ->
          nil

        {page, _} ->
          page
          |> List.last()
          |> then(fn event ->
            %{created_at: cursor_created_at(event.created_at), id: event.id}
            |> Jason.encode!()
            |> Base.url_encode64(padding: false)
          end)
      end

    {page, next_cursor}
  end

  defp format_event(event) do
    %{
      id: event.id,
      service: event.service,
      event: event.event,
      entity_type: event.entity_type,
      entity_ref: event.entity_ref,
      severity: event.severity,
      summary: event.summary,
      payload: Jason.decode!(event.payload),
      created_at: event.created_at
    }
  end

  defp summary_for(events, nil, window),
    do: "Returned #{length(events)} timeline events in the last #{window}."

  defp summary_for(events, service, window),
    do: "Returned #{length(events)} timeline events for #{service} in the last #{window}."

  defp cursor_created_at(%DateTime{} = created_at), do: DateTime.to_iso8601(created_at)
  defp cursor_created_at(created_at), do: created_at
end
