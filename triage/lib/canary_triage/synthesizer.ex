defmodule CanaryTriage.Synthesizer do
  @moduledoc """
  Webhook -> GitHub issue content.

  Health checks: deterministic templates (no LLM, no latency, no cost).
  Errors: LLM synthesis via OpenRouter for narrative + investigation steps.
  """

  require Logger

  @openrouter_url "https://openrouter.ai/api/v1/chat/completions"
  @model "google/gemini-3-flash-preview"

  @type issue :: %{
          String.t() => String.t() | [String.t()]
        }

  # --- Health check templates (deterministic, no LLM) ---

  @spec build_health_check_issue(map()) :: {:ok, issue()}
  def build_health_check_issue(payload) do
    state = payload["state"]
    service = get_in(payload, ["target", "name"]) || "unknown"
    url = get_in(payload, ["target", "url"])

    title = "Health Check #{format_state(state)}: #{service}"

    body = """
    ## Health Check #{format_state(state)}

    | Field | Value |
    |-------|-------|
    | **Service** | `#{service}` |
    | **State** | `#{state}` |
    | **Previous State** | `#{payload["previous_state"]}` |
    | **Target URL** | #{url} |
    | **Timestamp** | #{payload["timestamp"]} |
    | **Last Success** | #{payload["last_success_at"] || "N/A"} |

    ## Investigation Steps

    1. Check service health: `flyctl status --app #{service}`
    2. Review recent deploys: `flyctl releases --app #{service}`
    3. Check logs: `flyctl logs --app #{service}`
    4. Verify endpoint: `curl -I #{url}`
    """

    {:ok,
     %{
       "title" => title,
       "body" => body,
       "labels" => ["health-check", priority_label(state)],
       "priority" => priority_from_state(state)
     }}
  end

  @spec build_health_check_comment(map()) :: String.t()
  def build_health_check_comment(payload) do
    state = payload["state"]

    """
    ## Update: #{format_state(state)}

    | Field | Value |
    |-------|-------|
    | **State** | `#{state}` |
    | **Timestamp** | #{payload["timestamp"]} |
    """
  end

  @spec build_recovery_comment(map()) :: String.t()
  def build_recovery_comment(payload) do
    service = get_in(payload, ["target", "name"]) || "unknown"

    """
    ## Recovered

    `#{service}` has recovered.

    | Field | Value |
    |-------|-------|
    | **State** | `#{payload["state"]}` |
    | **Previous State** | `#{payload["previous_state"]}` |
    | **Timestamp** | #{payload["timestamp"]} |
    """
  end

  defp format_state("degraded"), do: "Degraded"
  defp format_state("down"), do: "Down"
  defp format_state("healthy"), do: "Recovered"
  defp format_state(state), do: String.capitalize(state)

  defp priority_label("down"), do: "critical"
  defp priority_label("degraded"), do: "high-priority"
  defp priority_label(_), do: "medium-priority"

  defp priority_from_state("down"), do: "critical"
  defp priority_from_state("degraded"), do: "high"
  defp priority_from_state(_), do: "medium"

  # --- Error synthesis (LLM-powered) ---

  @spec synthesize(map(), map() | nil) :: {:ok, issue()} | {:error, term()}
  def synthesize(webhook_payload, enriched_detail \\ nil) do
    api_key = Application.get_env(:canary_triage, :openrouter_api_key)

    prompt = build_prompt(webhook_payload, enriched_detail)

    body = %{
      model: @model,
      messages: [%{role: "user", content: prompt}],
      response_format: %{type: "json_object"}
    }

    case Req.post(@openrouter_url,
           json: body,
           headers: [{"authorization", "Bearer #{api_key}"}],
           receive_timeout: 30_000,
           finch: CanaryTriage.Finch
         ) do
      {:ok, %{status: 200, body: resp}} ->
        parse_response(resp)

      {:ok, %{status: status, body: resp}} ->
        Logger.error("OpenRouter API error: #{status} #{inspect(resp)}")
        {:error, {:openrouter, status}}

      {:error, reason} ->
        Logger.error("OpenRouter API request failed: #{inspect(reason)}")
        {:error, reason}
    end
  end

  defp build_prompt(payload, detail) do
    event = payload["event"] || "unknown"

    service =
      get_in(payload, ["error", "service"]) || get_in(payload, ["target", "name"]) || "unknown"

    context =
      case {event, detail} do
        {"error." <> _, %{"stack_trace" => st}} when is_binary(st) ->
          """
          Error class: #{get_in(payload, ["error", "error_class"])}
          Message: #{get_in(payload, ["error", "message"])}
          Service: #{service}
          Severity: #{get_in(payload, ["error", "severity"])}
          Group hash: #{get_in(payload, ["error", "group_hash"])}
          Occurrences: #{get_in(detail, ["group", "total_count"]) || 1}
          First seen: #{get_in(detail, ["group", "first_seen_at"]) || payload["timestamp"]}
          Last seen: #{get_in(detail, ["group", "last_seen_at"]) || payload["timestamp"]}
          Environment: #{detail["environment"] || "production"}

          Stack trace:
          #{st}

          Context: #{inspect(detail["context"])}
          """

        {"error." <> _, _} ->
          """
          Error class: #{get_in(payload, ["error", "error_class"])}
          Message: #{get_in(payload, ["error", "message"])}
          Service: #{service}
          Severity: #{get_in(payload, ["error", "severity"])}
          """

        _ ->
          inspect(payload, pretty: true)
      end

    """
    You are an expert incident responder creating a GitHub issue from an automated observability alert.

    Event type: #{event}
    Service: #{service}
    Timestamp: #{payload["timestamp"]}

    #{context}

    Create a GitHub issue. Respond with JSON matching this exact shape:
    {"title": "...", "body": "...", "labels": ["..."], "priority": "critical|high|medium|low"}

    Rules:
    1. Title: concise, specific (include error class or target name)
    2. Body: GitHub-flavored markdown with summary, impact, technical details, investigation steps. Put raw data in a <details> block.
    3. Labels from: bug, critical, high-priority, medium-priority, low-priority, health-check, error, regression, tls
    4. Priority: critical (production down / data loss), high (degraded service), medium (intermittent / new error class), low (warning / info severity)

    Be specific and actionable. Do not hallucinate information not in the data above.
    """
  end

  defp parse_response(resp) do
    text =
      get_in(resp, ["choices", Access.at(0), "message", "content"])

    case text do
      nil ->
        {:error, :no_content}

      json_str ->
        case Jason.decode(json_str) do
          {:ok, %{"title" => _, "body" => _, "labels" => _, "priority" => _} = issue} ->
            {:ok, issue}

          {:ok, other} ->
            Logger.error("Unexpected LLM response shape: #{inspect(other)}")
            {:error, :bad_shape}

          {:error, reason} ->
            Logger.error("Failed to parse LLM JSON: #{inspect(reason)}")
            {:error, :json_parse}
        end
    end
  end
end
