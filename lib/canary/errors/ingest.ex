defmodule Canary.Errors.Ingest do
  @moduledoc """
  Error ingest pipeline: validate → group → persist → webhook.
  Deep module: single public function, complex internal machinery.
  """

  alias Canary.Errors.{Classification, DedupCache, Grouping}
  alias Canary.{ID, Incidents, Repo, Timeline}
  alias Canary.Schemas.{Error, ErrorGroup}

  require Logger

  @max_context_size 8_192
  @max_fingerprint_elements 5
  @max_fingerprint_element_len 256

  @spec ingest(map()) :: {:ok, map()} | {:error, atom(), term()}
  def ingest(attrs) do
    with :ok <- validate_required(attrs),
         :ok <- validate_context(attrs),
         :ok <- validate_fingerprint(attrs) do
      {group_hash, template} = Grouping.compute_group_hash(attrs)
      classification = Classification.classify(attrs)
      now = DateTime.utc_now() |> DateTime.to_iso8601()
      error_id = ID.error_id()

      error_attrs = %{
        service: attrs["service"],
        error_class: attrs["error_class"],
        message: String.slice(attrs["message"], 0, 4_096),
        message_template: template,
        stack_trace: truncate(attrs["stack_trace"], 32_768),
        context: truncate_context(attrs["context"]),
        severity: attrs["severity"] || "error",
        environment: attrs["environment"] || "production",
        group_hash: group_hash,
        fingerprint: encode_fingerprint(attrs["fingerprint"]),
        region: attrs["region"],
        classification_category: Atom.to_string(classification.category),
        classification_persistence: Atom.to_string(classification.persistence),
        classification_component: Atom.to_string(classification.component),
        created_at: now
      }

      result =
        Repo.transaction(fn ->
          {:ok, error} =
            %Error{id: error_id}
            |> Error.changeset(error_attrs)
            |> Repo.insert()

          {is_new, is_regression} = upsert_group(error, group_hash, template, now)

          maybe_enqueue_webhooks(error, group_hash, is_new, is_regression)

          {error,
           %{
             id: error.id,
             group_hash: group_hash,
             is_new_class: is_new
           }}
        end)

      with {:ok, {error, summary}} <- result do
        broadcast_new_error(error)
        maybe_correlate_incident(summary.group_hash, error.service)
        {:ok, summary}
      end
    end
  end

  defp validate_required(attrs) do
    required = ~w(service error_class message)

    missing =
      Enum.filter(required, fn k ->
        val = attrs[k]
        is_nil(val) or val == ""
      end)

    case missing do
      [] -> :ok
      fields -> {:error, :validation_error, Enum.map(fields, &{&1, ["can't be blank"]})}
    end
  end

  defp validate_context(%{"context" => ctx}) when is_map(ctx) do
    json = Jason.encode!(ctx)

    if byte_size(json) > @max_context_size do
      {:error, :payload_too_large, "context exceeds #{@max_context_size} bytes"}
    else
      :ok
    end
  end

  defp validate_context(_), do: :ok

  defp validate_fingerprint(%{"fingerprint" => fp}) when is_list(fp) do
    cond do
      length(fp) > @max_fingerprint_elements ->
        {:error, :validation_error,
         %{"fingerprint" => ["max #{@max_fingerprint_elements} elements"]}}

      Enum.any?(fp, &(not is_binary(&1))) ->
        {:error, :validation_error, %{"fingerprint" => ["elements must be strings"]}}

      Enum.any?(fp, &(String.length(&1) > @max_fingerprint_element_len)) ->
        {:error, :validation_error,
         %{"fingerprint" => ["elements max #{@max_fingerprint_element_len} chars"]}}

      true ->
        :ok
    end
  end

  defp validate_fingerprint(%{"fingerprint" => _}) do
    {:error, :validation_error, %{"fingerprint" => ["must be a list of strings"]}}
  end

  defp validate_fingerprint(_), do: :ok

  defp upsert_group(error, group_hash, template, now) do
    case Repo.get(ErrorGroup, group_hash) do
      nil ->
        %ErrorGroup{group_hash: group_hash}
        |> ErrorGroup.changeset(%{
          service: error.service,
          error_class: error.error_class,
          message_template: template,
          severity: error.severity,
          first_seen_at: now,
          last_seen_at: now,
          total_count: 1,
          last_error_id: error.id
        })
        |> Repo.insert!()

        {true, false}

      group ->
        last_seen = group.last_seen_at
        is_regression = regression?(last_seen, now)

        group
        |> ErrorGroup.changeset(%{
          last_seen_at: now,
          total_count: group.total_count + 1,
          last_error_id: error.id,
          status: "active"
        })
        |> Repo.update!()

        {false, is_regression}
    end
  end

  defp regression?(last_seen, now) do
    with {:ok, last_dt, _} <- DateTime.from_iso8601(last_seen),
         {:ok, now_dt, _} <- DateTime.from_iso8601(now) do
      DateTime.diff(now_dt, last_dt, :hour) >= 24
    else
      _ -> false
    end
  end

  defp maybe_enqueue_webhooks(error, group_hash, is_new, is_regression) do
    cond do
      is_new and not DedupCache.seen_recently?(group_hash) ->
        DedupCache.mark(group_hash)
        enqueue_error_webhook("error.new_class", error, group_hash)

      is_regression ->
        enqueue_error_webhook("error.regression", error, group_hash)

      true ->
        :ok
    end
  end

  defp enqueue_error_webhook(event, error, group_hash) do
    case Timeline.record_error(event, error, group_hash) do
      {:ok, payload} ->
        Canary.Workers.WebhookDelivery.enqueue_for_event(event, payload)

      {:error, reason} ->
        Logger.error(
          "Failed to record error event #{event} for #{group_hash}: #{inspect(reason)}"
        )
    end
  end

  defp broadcast_new_error(error) do
    Phoenix.PubSub.broadcast(Canary.PubSub, "errors:new", {:new_error, error})
  end

  defp maybe_correlate_incident(group_hash, service) do
    case safe_correlate_incident(:error_group, group_hash, service) do
      {:ok, _incident} ->
        :ok

      {:error, reason} ->
        Logger.error(
          "Failed to correlate incident for error group #{group_hash}: #{correlation_error_tag(reason)}"
        )
    end
  end

  defp safe_correlate_incident(signal_type, signal_ref, service) do
    Incidents.correlate(signal_type, signal_ref, service)
  rescue
    error ->
      {:error, {:exception, error.__struct__}}
  catch
    kind, reason ->
      {:error, {kind, reason}}
  end

  defp correlation_error_tag({:exception, module}) when is_atom(module),
    do: Atom.to_string(module)

  defp correlation_error_tag({kind, reason}), do: "#{kind}:#{correlation_error_tag(reason)}"
  defp correlation_error_tag(reason) when is_atom(reason), do: Atom.to_string(reason)
  defp correlation_error_tag(%module{}) when is_atom(module), do: Atom.to_string(module)
  defp correlation_error_tag(_reason), do: "unexpected"

  defp truncate(nil, _max), do: nil
  defp truncate(str, max), do: String.slice(str, 0, max)

  defp truncate_context(nil), do: nil

  defp truncate_context(ctx) when is_map(ctx) do
    json = Jason.encode!(ctx)
    if byte_size(json) <= @max_context_size, do: json, else: nil
  end

  defp truncate_context(ctx) when is_binary(ctx), do: ctx

  defp encode_fingerprint(nil), do: nil
  defp encode_fingerprint(fp) when is_list(fp), do: Jason.encode!(fp)
end
