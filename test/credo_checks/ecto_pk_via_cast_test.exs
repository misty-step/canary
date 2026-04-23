defmodule Canary.Checks.EctoPKViaCastTest do
  use Credo.Test.Case, async: false

  alias Canary.Checks.EctoPKViaCast

  setup_all do
    Application.ensure_all_started(:credo)
    :ok
  end

  defmodule AllowsPrimaryKeyCast do
    use Ecto.Schema

    import Ecto.Changeset

    @primary_key {:slug, :string, autogenerate: false}
    schema "allows_primary_key_cast" do
      field :name, :string
    end

    def changeset(row, attrs) do
      cast(row, attrs, [:slug, :name])
    end
  end

  test "reports literal custom primary keys passed through changeset attrs" do
    """
    defmodule Sample do
      def insert_group do
        %Canary.Schemas.ErrorGroup{}
        |> Canary.Schemas.ErrorGroup.changeset(%{
          group_hash: "grp",
          service: "test-svc"
        })
      end
    end
    """
    |> to_source_file()
    |> run_check(EctoPKViaCast)
    |> assert_issue(fn issue ->
      assert issue.message =~ "Custom primary key `:group_hash`"
      assert issue.message =~ "CLAUDE.md footgun #1"
      assert issue.trigger == "group_hash"
    end)
  end

  test "reports local defaults maps merged into changeset attrs" do
    """
    defmodule Sample do
      alias Canary.Schemas.{ErrorGroup, Target}

      def insert_group(attrs) do
        defaults = %{
          group_hash: "grp",
          service: "test-svc"
        }

        %ErrorGroup{}
        |> ErrorGroup.changeset(Map.merge(defaults, attrs))
      end

      def insert_target(attrs) do
        defaults = %{url: "https://example.com", name: "api"}

        %Target{}
        |> Target.changeset(Map.merge(defaults, attrs))
      end
    end
    """
    |> to_source_file()
    |> run_check(EctoPKViaCast)
    |> assert_issue(%{trigger: "group_hash"})
  end

  test "accepts the custom primary key set on the struct" do
    """
    defmodule Sample do
      alias Canary.Schemas.ErrorGroup

      def insert_group(attrs) do
        group_hash = "grp"
        defaults = %{group_hash: group_hash, service: "test-svc"}

        %ErrorGroup{group_hash: group_hash}
        |> ErrorGroup.changeset(Map.merge(defaults, attrs))
      end
    end
    """
    |> to_source_file()
    |> run_check(EctoPKViaCast)
    |> refute_issues()
  end

  test "accepts schemas whose changeset casts the custom primary key" do
    """
    defmodule Sample do
      alias Canary.Checks.EctoPKViaCastTest.AllowsPrimaryKeyCast

      def insert_row do
        %AllowsPrimaryKeyCast{}
        |> AllowsPrimaryKeyCast.changeset(%{slug: "row-1", name: "ok"})
      end
    end
    """
    |> to_source_file()
    |> run_check(EctoPKViaCast)
    |> refute_issues()
  end

  test "ignores unrelated code" do
    """
    defmodule Sample do
      def run(attrs) do
        attrs
        |> Map.merge(%{group_hash: "grp"})
      end
    end
    """
    |> to_source_file()
    |> run_check(EctoPKViaCast)
    |> refute_issues()
  end

  test "reports source-only schemas via static metadata" do
    [
      """
      defmodule Fixture.Schema do
        use Ecto.Schema
        import Ecto.Changeset

        @primary_key {:slug, :string, autogenerate: false}
        schema "fixture_rows" do
          field :name, :string
        end

        @required ~w(name)a

        def changeset(row, attrs) do
          row
          |> cast(attrs, @required)
        end
      end
      """,
      """
      defmodule Fixture.UseSchema do
        def insert_row do
          %Fixture.Schema{}
          |> Fixture.Schema.changeset(%{slug: "row-1", name: "ok"})
        end
      end
      """
    ]
    |> to_source_files()
    |> run_check(EctoPKViaCast)
    |> assert_issue(%{trigger: "slug"})
  end
end
