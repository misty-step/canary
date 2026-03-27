defmodule CanaryTriage.GitHubTest do
  use ExUnit.Case, async: true

  alias CanaryTriage.GitHub

  setup do
    Application.put_env(:canary_triage, :service_repos, %{"my-service" => "misty-step/canary"})
    on_exit(fn -> Application.delete_env(:canary_triage, :service_repos) end)
    :ok
  end

  defp stub_github(handler), do: Req.Test.stub(CanaryTriage.GitHub, handler)

  describe "find_open_health_issue/1" do
    test "returns issue when matching title found" do
      stub_github(fn conn ->
        Req.Test.json(conn, [
          %{
            "number" => 42,
            "title" => "Health Check Degraded: my-service",
            "html_url" => "https://github.com/misty-step/canary/issues/42"
          }
        ])
      end)

      assert {:ok, %{"number" => 42}} = GitHub.find_open_health_issue("my-service")
    end

    test "returns :not_found when no matching issues" do
      stub_github(fn conn -> Req.Test.json(conn, []) end)

      assert :not_found = GitHub.find_open_health_issue("my-service")
    end

    test "filters by service name in title" do
      stub_github(fn conn ->
        Req.Test.json(conn, [
          %{"number" => 1, "title" => "Health Check Degraded: other-service"}
        ])
      end)

      assert :not_found = GitHub.find_open_health_issue("my-service")
    end

    test "does not match substring service names" do
      stub_github(fn conn ->
        Req.Test.json(conn, [
          %{"number" => 1, "title" => "Health Check Degraded: my-service-api"}
        ])
      end)

      assert :not_found = GitHub.find_open_health_issue("my-service")
    end

    test "returns error on API failure" do
      stub_github(fn conn ->
        conn
        |> Plug.Conn.put_status(500)
        |> Req.Test.json(%{"message" => "Internal Server Error"})
      end)

      assert {:error, {:github, 500, _}} = GitHub.find_open_health_issue("my-service")
    end
  end

  describe "close_issue/3" do
    test "comments then closes" do
      calls = :counters.new(1, [:atomics])

      stub_github(fn conn ->
        case conn.method do
          "POST" ->
            :counters.add(calls, 1, 1)
            conn |> Plug.Conn.put_status(201) |> Req.Test.json(%{"id" => 99})

          "PATCH" ->
            assert :counters.get(calls, 1) == 1
            Req.Test.json(conn, %{"number" => 42, "state" => "closed"})
        end
      end)

      assert {:ok, %{"state" => "closed"}} = GitHub.close_issue("my-service", 42, "Closing")
    end

    test "aborts if comment fails" do
      stub_github(fn conn ->
        assert conn.method == "POST"
        conn |> Plug.Conn.put_status(403) |> Req.Test.json(%{"message" => "Forbidden"})
      end)

      assert {:error, {:github, 403, _}} = GitHub.close_issue("my-service", 42, "Closing")
    end
  end

  describe "comment_on_issue/3" do
    test "posts comment and returns response" do
      stub_github(fn conn ->
        assert conn.method == "POST"
        conn |> Plug.Conn.put_status(201) |> Req.Test.json(%{"id" => 99, "body" => "test"})
      end)

      assert {:ok, %{"id" => 99}} = GitHub.comment_on_issue("my-service", 42, "test comment")
    end

    test "returns error on non-201 status" do
      stub_github(fn conn ->
        conn |> Plug.Conn.put_status(404) |> Req.Test.json(%{"message" => "Not Found"})
      end)

      assert {:error, {:github, 404, _}} = GitHub.comment_on_issue("my-service", 42, "test")
    end
  end

  describe "create_issue/2" do
    test "creates issue and returns response" do
      stub_github(fn conn ->
        assert conn.method == "POST"

        conn
        |> Plug.Conn.put_status(201)
        |> Req.Test.json(%{"number" => 10, "html_url" => "https://example.com"})
      end)

      issue = %{"title" => "Test", "body" => "Body", "labels" => ["bug"]}
      assert {:ok, %{"number" => 10}} = GitHub.create_issue("my-service", issue)
    end
  end
end
