defmodule CanaryTriage.GitHub do
  @moduledoc """
  Creates GitHub issues via the REST API.
  Maps service names to repos via configuration.
  """

  require Logger

  @spec create_issue(String.t(), map()) :: {:ok, map()} | {:error, term()}
  def create_issue(service, %{"title" => title, "body" => body, "labels" => labels} = _issue) do
    repo = resolve_repo(service)
    token = Application.get_env(:canary_triage, :github_token)

    case Req.post("https://api.github.com/repos/#{repo}/issues",
           json: %{title: title, body: body, labels: labels},
           headers: [
             {"authorization", "Bearer #{token}"},
             {"accept", "application/vnd.github+json"},
             {"x-github-api-version", "2022-11-28"}
           ],
           receive_timeout: 15_000,
           finch: CanaryTriage.Finch
         ) do
      {:ok, %{status: 201, body: resp}} ->
        Logger.info("Created issue ##{resp["number"]} in #{repo}: #{title}")
        {:ok, resp}

      {:ok, %{status: status, body: resp}} ->
        Logger.error("GitHub API error #{status}: #{inspect(resp)}")
        {:error, {:github, status, resp}}

      {:error, reason} ->
        Logger.error("GitHub API request failed: #{inspect(reason)}")
        {:error, reason}
    end
  end

  defp resolve_repo(service) do
    service_repos = Application.get_env(:canary_triage, :service_repos, %{})
    org = Application.get_env(:canary_triage, :github_org, "misty-step")

    case Map.get(service_repos, service) do
      nil -> "#{org}/#{service}"
      repo -> repo
    end
  end
end
