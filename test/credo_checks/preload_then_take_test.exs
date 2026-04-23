defmodule Canary.Checks.PreloadThenTakeTest do
  use Credo.Test.Case, async: true

  alias Canary.Checks.PreloadThenTake

  setup_all do
    Application.ensure_all_started(:credo)
    :ok
  end

  test "reports preloaded associations truncated in memory in read models" do
    """
    defmodule Canary.Query.Sample do
      def detail(incident) do
        Canary.Repo.preload(incident, :signals)
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(fn issue ->
      assert issue.message =~ "preload on `:signals`"
      assert issue.message =~ "loads every row into memory"
      assert issue.message =~ "fetch_top_signals/3"
      assert issue.trigger == "signals"
    end)
  end

  test "reports field extraction followed by take" do
    """
    defmodule Canary.Query.Sample do
      def signals(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> Map.get(:signals)
        |> Enum.slice(0, 25)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports issues in the root read-model module" do
    """
    defmodule Canary.Query do
      def signals(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> Map.get(:signals)
        |> Enum.take(25)
      end
    end
    """
    |> to_source_file("lib/canary/query.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports Ecto query preload macros followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(repo, id) do
        Canary.Schemas.Incident
        |> where([i], i.id == ^id)
        |> preload([i], signals: i.signals)
        |> repo.one()
        |> Map.update!(:signals, &Stream.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "ignores local helper functions named preload" do
    """
    defmodule Canary.Query.Sample do
      def signals(incident) do
        incident
        |> preload(:signals)
        |> Map.get(:signals)
        |> Enum.take(25)
      end

      defp preload(incident, _field), do: incident
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "ignores non-Repo modules with preload functions" do
    """
    defmodule Canary.Query.Sample do
      def signals(cache, incident) do
        cache.preload(incident, :signals)
        |> Map.get(:signals)
        |> Enum.take(25)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "ignores incidental mentions of the preloaded field" do
    """
    defmodule Canary.Query.Sample do
      def events(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> Map.put(:metadata, %{note: "signals"})
        |> Map.get(:events)
        |> Enum.take(25)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "accepts preload queries with a SQL limit" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(incident, limit) do
        Canary.Repo.preload(
          incident,
          signals: from(s in Canary.Schemas.IncidentSignal, order_by: s.id, limit: ^limit)
        )
        |> Map.update!(:signals, &Enum.take(&1, limit))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "ignores truncation outside read-model paths" do
    """
    defmodule CanaryWeb.SampleController do
      def show(incident) do
        Canary.Repo.preload(incident, :signals)
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary_web/sample_controller.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "accepts bounded streams" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def recent(repo) do
        from(e in Canary.Schemas.ErrorGroup, order_by: e.last_seen_at)
        |> repo.stream(max_rows: 25)
        |> Stream.take(25)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end
end
