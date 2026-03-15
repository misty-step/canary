defmodule CanaryTriage.CanaryClient do
  @moduledoc """
  Fetches enriched error/incident detail from Canary's query API.
  Used to enrich webhook payloads before LLM synthesis.
  """

  @spec fetch_error_detail(String.t()) :: {:ok, map()} | {:error, term()}
  def fetch_error_detail(error_id) do
    endpoint = Application.get_env(:canary_triage, :canary_endpoint)
    api_key = Application.get_env(:canary_triage, :canary_api_key)

    case Req.get("#{endpoint}/api/v1/errors/#{error_id}",
           headers: [{"authorization", "Bearer #{api_key}"}],
           receive_timeout: 10_000,
           finch: CanaryTriage.Finch
         ) do
      {:ok, %{status: 200, body: body}} -> {:ok, body}
      {:ok, %{status: status}} -> {:error, {:http, status}}
      {:error, reason} -> {:error, reason}
    end
  end

  @spec fetch_health_status() :: {:ok, map()} | {:error, term()}
  def fetch_health_status do
    endpoint = Application.get_env(:canary_triage, :canary_endpoint)
    api_key = Application.get_env(:canary_triage, :canary_api_key)

    case Req.get("#{endpoint}/api/v1/health-status",
           headers: [{"authorization", "Bearer #{api_key}"}],
           receive_timeout: 10_000,
           finch: CanaryTriage.Finch
         ) do
      {:ok, %{status: 200, body: body}} -> {:ok, body}
      {:ok, %{status: status}} -> {:error, {:http, status}}
      {:error, reason} -> {:error, reason}
    end
  end
end
