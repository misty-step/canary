defmodule CanaryTriage.GitHub do
  @moduledoc """
  GitHub issue lifecycle: create, search, comment, close.
  Maps service names to repos via configuration.
  """

  require Logger

  @spec create_issue(String.t(), map()) :: {:ok, map()} | {:error, term()}
  def create_issue(service, %{"title" => title, "body" => body} = issue) do
    labels = Map.get(issue, "labels", [])
    repo = resolve_repo(service)

    case Req.post(
           "https://api.github.com/repos/#{repo}/issues",
           [json: %{title: title, body: body, labels: labels}] ++ common_opts()
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

  @spec find_open_health_issue(String.t()) :: {:ok, map()} | :not_found | {:error, term()}
  def find_open_health_issue(service) do
    repo = resolve_repo(service)

    case Req.get(
           "https://api.github.com/repos/#{repo}/issues",
           [params: [labels: "health-check", state: "open", per_page: 10]] ++ common_opts()
         ) do
      {:ok, %{status: 200, body: issues}} when is_list(issues) ->
        case Enum.find(issues, &String.ends_with?(&1["title"], ": #{service}")) do
          nil -> :not_found
          issue -> {:ok, issue}
        end

      {:ok, %{status: status, body: resp}} ->
        Logger.error("GitHub API error #{status}: #{inspect(resp)}")
        {:error, {:github, status, resp}}

      {:error, reason} ->
        Logger.error("GitHub API request failed: #{inspect(reason)}")
        {:error, reason}
    end
  end

  @spec close_issue(String.t(), integer(), String.t()) :: {:ok, map()} | {:error, term()}
  def close_issue(service, issue_number, comment) do
    repo = resolve_repo(service)

    with {:ok, _} <- comment_on_issue(service, issue_number, comment) do
      case Req.patch(
             "https://api.github.com/repos/#{repo}/issues/#{issue_number}",
             [json: %{state: "closed", state_reason: "completed"}] ++ common_opts()
           ) do
        {:ok, %{status: 200, body: resp}} ->
          Logger.info("Closed issue ##{issue_number} in #{repo}")
          {:ok, resp}

        {:ok, %{status: status, body: resp}} ->
          Logger.error("GitHub API error #{status}: #{inspect(resp)}")
          {:error, {:github, status, resp}}

        {:error, reason} ->
          Logger.error("GitHub API request failed: #{inspect(reason)}")
          {:error, reason}
      end
    end
  end

  @spec comment_on_issue(String.t(), integer(), String.t()) :: {:ok, map()} | {:error, term()}
  def comment_on_issue(service, issue_number, comment) do
    repo = resolve_repo(service)

    case Req.post(
           "https://api.github.com/repos/#{repo}/issues/#{issue_number}/comments",
           [json: %{body: comment}] ++ common_opts()
         ) do
      {:ok, %{status: 201, body: resp}} ->
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

  defp common_opts do
    token = Application.get_env(:canary_triage, :github_token)

    [
      headers: [
        {"authorization", "Bearer #{token}"},
        {"accept", "application/vnd.github+json"},
        {"x-github-api-version", "2022-11-28"}
      ],
      receive_timeout: 15_000
    ] ++ Application.get_env(:canary_triage, :github_req_options, [])
  end
end
