defmodule Canary.ServiceOnboarding.Payload do
  @moduledoc false

  alias Canary.TargetResponse
  alias Canary.ServiceOnboarding.Connect.Result

  @verification_read_key "$CANARY_READ_KEY"

  @spec render(Result.t(), String.t()) :: map()
  def render(%Result{} = result, base_url) do
    %{request: request, target: target, api_key: api_key, raw_key: raw_key} = result

    %{
      service: request.service,
      api_key: %{
        id: api_key.id,
        name: api_key.name,
        scope: api_key.scope,
        key: raw_key,
        key_prefix: api_key.key_prefix,
        created_at: api_key.created_at,
        warning: "Store this key securely. It will not be shown again."
      },
      target: TargetResponse.render(target),
      links: %{
        dashboard: "#{base_url}/dashboard",
        report: "#{base_url}/api/v1/report?window=1h",
        service_query:
          "#{base_url}/api/v1/query?service=#{URI.encode_www_form(request.service)}&window=1h"
      },
      snippets: %{
        error_ingest_curl: error_ingest_curl(base_url, raw_key, request),
        report_curl: report_curl(base_url, raw_key),
        service_query_curl: service_query_curl(base_url, raw_key, request.service),
        elixir_logger: elixir_logger_snippet(base_url, raw_key, request),
        typescript_init: typescript_init_snippet(base_url, raw_key, request)
      }
    }
  end

  defp error_ingest_curl(base_url, raw_key, request) do
    payload =
      Jason.encode!(%{
        service: request.service,
        environment: request.environment,
        error_class: "RuntimeError",
        message: "canary onboarding check",
        severity: "error",
        context: %{
          source: "service-onboarding"
        }
      })

    """
    curl -X POST #{base_url}/api/v1/errors \\
      -H "Authorization: Bearer #{raw_key}" \\
      -H "Content-Type: application/json" \\
      -d @- <<'JSON'
    #{payload}
    JSON
    """
    |> String.trim()
  end

  defp report_curl(base_url, _raw_key) do
    """
    curl "#{base_url}/api/v1/report?window=1h" \\
      -H "Authorization: Bearer #{@verification_read_key}"
    """
    |> String.trim()
  end

  defp service_query_curl(base_url, _raw_key, service) do
    """
    curl "#{base_url}/api/v1/query?service=#{URI.encode_www_form(service)}&window=1h" \\
      -H "Authorization: Bearer #{@verification_read_key}"
    """
    |> String.trim()
  end

  defp elixir_logger_snippet(base_url, raw_key, request) do
    """
    CanarySdk.attach(
      endpoint: "#{base_url}",
      api_key: "#{raw_key}",
      service: "#{request.service}",
      environment: "#{request.environment}"
    )
    """
    |> String.trim()
  end

  defp typescript_init_snippet(base_url, raw_key, request) do
    """
    import { initCanary } from "@canary-obs/sdk";

    initCanary({
      endpoint: "#{base_url}",
      apiKey: "#{raw_key}",
      service: "#{request.service}",
      environment: "#{request.environment}"
    });
    """
    |> String.trim()
  end
end
