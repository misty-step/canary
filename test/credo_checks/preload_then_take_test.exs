defmodule Canary.Checks.PreloadThenTakeTest do
  use Credo.Test.Case, async: true

  alias Canary.Checks.PreloadThenTake

  setup_all do
    Application.ensure_all_started(:credo)
    :ok
  end

  test "custom check is wired into Credo config" do
    {config, _bindings} = Code.eval_file(Path.expand(".credo.exs"))

    assert Enum.any?(config.configs, fn config ->
             {Canary.Checks.PreloadThenTake, []} in config.checks.enabled
           end)

    assert Enum.any?(config.configs, fn config ->
             Enum.any?(config.requires, &String.ends_with?(&1, "preload_then_take.ex"))
           end)
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

  test "reports truncation before later pipeline stages" do
    """
    defmodule Canary.Query.Sample do
      def signals(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> Map.get(:signals)
        |> Enum.take(25)
        |> Enum.reverse()
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

  test "reports Ecto query preload shorthand followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(repo, id) do
        Canary.Schemas.Incident
        |> where([i], i.id == ^id)
        |> preload(:signals)
        |> repo.one()
        |> Map.update!(:signals, &Stream.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports direct Ecto preload shorthand followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(repo, id) do
        preload(Canary.Schemas.Incident, :signals)
        |> where([i], i.id == ^id)
        |> repo.one()
        |> Map.update!(:signals, &Stream.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports qualified Ecto preload followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      def detail(repo, id) do
        Canary.Schemas.Incident
        |> Ecto.Query.where([i], i.id == ^id)
        |> Ecto.Query.preload(:signals)
        |> repo.one()
        |> Map.update!(:signals, &Stream.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports aliased Ecto preload followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      alias Ecto.Query, as: EQ

      def detail(repo, id) do
        Canary.Schemas.Incident
        |> EQ.where([i], i.id == ^id)
        |> EQ.preload(:signals)
        |> repo.one()
        |> Map.update!(:signals, &Stream.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports Repo.preload/3 followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      def detail(incident) do
        Canary.Repo.preload(incident, :signals, force: true)
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports piped Repo.preload/3 followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      def detail(incident) do
        incident
        |> Canary.Repo.preload(:signals, force: true)
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports direct Ecto preload macros followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(repo, id) do
        preload(Canary.Schemas.Incident, [i], signals: i.signals)
        |> where([i], i.id == ^id)
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

  test "ignores local helper functions named preload when Ecto.Query is imported" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

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

  test "does not leak imported Ecto.Query into sibling modules" do
    """
    defmodule Canary.Query.WithImport do
      import Ecto.Query

      def detail(repo) do
        Canary.Schemas.Incident
        |> where([i], i.id != nil)
        |> repo.all()
      end
    end

    defmodule Canary.Query.Sample do
      def signals(incident) do
        incident
        |> preload(:signals)
        |> Map.get(:signals)
        |> Enum.take(25)
      end
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

  test "ignores fields referenced only after truncation" do
    """
    defmodule Canary.Query.Sample do
      def events(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> Map.get(:events)
        |> Enum.take(25)
        |> then(fn events -> %{events: events, signals: incident.signals} end)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "ignores fields referenced before unrelated truncation" do
    """
    defmodule Canary.Query.Sample do
      def events(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> then(fn incident -> %{signals: incident.signals, events: incident.events} end)
        |> Map.get(:events)
        |> Enum.take(25)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "reports extracted preloaded fields transformed before truncation" do
    """
    defmodule Canary.Query.Sample do
      def signals(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> Map.get(:signals)
        |> Enum.filter(& &1.active)
        |> Enum.take(25)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "ignores same-stage truncation when it does not operate on the preloaded field" do
    """
    defmodule Canary.Query.Sample do
      def events(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> then(fn incident ->
          %{signals: incident.signals, top_events: Enum.take(incident.events, 25)}
        end)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "reports same-stage truncation when it operates on the preloaded field" do
    """
    defmodule Canary.Query.Sample do
      def signals(incident) do
        incident
        |> Canary.Repo.preload(:signals)
        |> then(fn incident -> Enum.take(incident.signals, 25) end)
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
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

  test "reports preload queries without a SQL limit" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(incident) do
        Canary.Repo.preload(
          incident,
          signals: from(s in Canary.Schemas.IncidentSignal, order_by: s.id)
        )
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports piped preload queries without a SQL limit" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(incident) do
        Canary.Repo.preload(
          incident,
          signals:
            from(s in Canary.Schemas.IncidentSignal, order_by: s.id)
            |> where([s], s.id > 0)
        )
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports pinned inline preload queries without a SQL limit" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(incident) do
        Canary.Repo.preload(
          incident,
          signals: ^from(s in Canary.Schemas.IncidentSignal, order_by: s.id)
        )
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "accepts preload queries with a qualified SQL limit" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(incident, limit) do
        signal_query =
          from(s in Canary.Schemas.IncidentSignal, order_by: s.id)
          |> Ecto.Query.limit(^limit)

        Canary.Repo.preload(incident, signals: signal_query)
        |> Map.update!(:signals, &Enum.take(&1, limit))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "accepts preload query variables as statically unresolved" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(incident, limit) do
        signal_query =
          from(s in Canary.Schemas.IncidentSignal, order_by: s.id, limit: ^limit)

        Canary.Repo.preload(incident, signals: signal_query)
        |> Map.update!(:signals, &Enum.take(&1, limit))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "accepts piped preload query variables as statically unresolved" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(incident, limit) do
        signal_query =
          from(s in Canary.Schemas.IncidentSignal, order_by: s.id, limit: ^limit)

        Canary.Repo.preload(
          incident,
          signals: signal_query |> where([s], s.id > 0)
        )
        |> Map.update!(:signals, &Enum.take(&1, limit))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> refute_issues()
  end

  test "reports preload queries with incidental limit metadata" do
    """
    defmodule Canary.Query.Sample do
      def detail(incident) do
        Canary.Repo.preload(
          incident,
          signals: %{metadata: %{limit: 25}}
        )
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
  end

  test "reports from preload clauses followed by in-memory truncation" do
    """
    defmodule Canary.Query.Sample do
      import Ecto.Query

      def detail(repo, id) do
        from(i in Canary.Schemas.Incident,
          where: i.id == ^id,
          preload: [signals: i.signals]
        )
        |> repo.one()
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/query/sample.ex")
    |> run_check(PreloadThenTake)
    |> assert_issue(%{trigger: "signals"})
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

  test "ignores query-named files outside the read-model directory" do
    """
    defmodule Canary.Reporting.QueryHelper do
      def show(incident) do
        Canary.Repo.preload(incident, :signals)
        |> Map.update!(:signals, &Enum.take(&1, 25))
      end
    end
    """
    |> to_source_file("lib/canary/reporting/query_helper.ex")
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
