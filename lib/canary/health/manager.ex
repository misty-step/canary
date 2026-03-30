defmodule Canary.Health.Manager do
  @moduledoc """
  Loads active targets from DB on boot, starts one checker per target
  via DynamicSupervisor. Handles target CRUD lifecycle.
  """

  use GenServer

  alias Canary.{ID, Repo}
  alias Canary.Schemas.{Target, TargetState}
  alias Canary.Health.Supervisor, as: HealthSup
  import Ecto.Query

  require Logger

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @spec add_target(map()) :: {:ok, Target.t()} | {:error, Ecto.Changeset.t()}
  def add_target(attrs) do
    GenServer.call(__MODULE__, {:add_target, attrs})
  end

  @spec remove_target(String.t()) :: {:ok, Target.t()} | {:error, :not_found | Ecto.Changeset.t()}
  def remove_target(target_id) do
    GenServer.call(__MODULE__, {:remove_target, target_id})
  end

  @spec pause_target(String.t()) :: :ok | {:error, :not_found}
  def pause_target(target_id) do
    GenServer.call(__MODULE__, {:pause_target, target_id})
  end

  @spec resume_target(String.t()) :: :ok | {:error, :not_found}
  def resume_target(target_id) do
    GenServer.call(__MODULE__, {:resume_target, target_id})
  end

  @spec list_targets() :: [Target.t()]
  def list_targets do
    from(t in Target, order_by: t.name)
    |> Canary.Repos.read_repo().all()
  end

  # --- Callbacks ---

  @impl true
  def init(_opts) do
    send(self(), :boot)
    {:ok, %{}}
  end

  @impl true
  def handle_info(:boot, state) do
    targets = from(t in Target, where: t.active == 1) |> Repo.all()
    Logger.info("Starting #{length(targets)} health checkers")

    Enum.each(targets, fn target ->
      ensure_target_state(target.id)
      HealthSup.start_checker(target)
    end)

    {:noreply, state}
  rescue
    e ->
      Logger.warning("Health manager boot failed: #{inspect(e)}, retrying in 5s")
      Process.send_after(self(), :boot, 5_000)
      {:noreply, state}
  end

  @impl true
  def handle_call({:add_target, attrs}, _from, state) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    attrs =
      attrs
      |> Map.put("id", ID.target_id())
      |> Map.put("created_at", now)
      |> Map.put_new("service", Map.get(attrs, "name"))
      |> Map.put_new("method", "GET")
      |> Map.put_new("interval_ms", 60_000)
      |> Map.put_new("timeout_ms", 10_000)
      |> Map.put_new("expected_status", "200")
      |> Map.put_new("degraded_after", 1)
      |> Map.put_new("down_after", 3)
      |> Map.put_new("up_after", 1)
      |> Map.put_new("active", 1)

    atom_attrs = atomize_keys(attrs)
    id = atom_attrs[:id] || attrs["id"]

    case %Target{id: id} |> Target.changeset(Map.drop(atom_attrs, [:id])) |> Repo.insert() do
      {:ok, target} ->
        ensure_target_state(target.id)
        HealthSup.start_checker(target)
        {:reply, {:ok, target}, state}

      {:error, cs} ->
        {:reply, {:error, cs}, state}
    end
  end

  @impl true
  def handle_call({:remove_target, target_id}, _from, state) do
    HealthSup.stop_checker(target_id)

    case Repo.get(Target, target_id) do
      nil -> {:reply, {:error, :not_found}, state}
      target -> {:reply, Repo.delete(target), state}
    end
  end

  @impl true
  def handle_call({:pause_target, target_id}, _from, state) do
    HealthSup.stop_checker(target_id)

    case Repo.get(Target, target_id) do
      nil ->
        {:reply, {:error, :not_found}, state}

      target ->
        target |> Target.changeset(%{active: 0}) |> Repo.update()

        case Repo.get(TargetState, target_id) do
          nil -> :ok
          ts -> ts |> TargetState.changeset(%{state: "paused"}) |> Repo.update!()
        end

        {:reply, :ok, state}
    end
  end

  @impl true
  def handle_call({:resume_target, target_id}, _from, state) do
    case Repo.get(Target, target_id) do
      nil ->
        {:reply, {:error, :not_found}, state}

      target ->
        target |> Target.changeset(%{active: 1}) |> Repo.update!()

        case Repo.get(TargetState, target_id) do
          nil -> :ok
          ts -> ts |> TargetState.changeset(%{state: "unknown"}) |> Repo.update!()
        end

        HealthSup.start_checker(%{target | active: 1})
        {:reply, :ok, state}
    end
  end

  defp ensure_target_state(target_id) do
    case Repo.get(TargetState, target_id) do
      nil ->
        %TargetState{target_id: target_id}
        |> TargetState.changeset(%{state: "unknown"})
        |> Repo.insert!()

      _ ->
        :ok
    end
  end

  defp atomize_keys(map) do
    Map.new(map, fn
      {k, v} when is_binary(k) -> {String.to_existing_atom(k), v}
      {k, v} -> {k, v}
    end)
  rescue
    _ -> map
  end
end
