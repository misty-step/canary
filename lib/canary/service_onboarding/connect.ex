defmodule Canary.ServiceOnboarding.Connect do
  @moduledoc false

  alias Canary.{Auth, ID, Repo}
  alias Canary.Health.Manager
  alias Canary.Schemas.{ApiKey, Target}
  alias Canary.ServiceOnboarding.Request
  alias Ecto.Changeset

  defmodule Result do
    @enforce_keys [:request, :target, :api_key, :raw_key]
    defstruct [:request, :target, :api_key, :raw_key]

    @type t :: %__MODULE__{
            request: Request.t(),
            target: Target.t(),
            api_key: ApiKey.t(),
            raw_key: String.t()
          }
  end

  @spec connect(Request.t(), keyword()) :: {:ok, Result.t()} | {:error, Changeset.t() | :internal}
  def connect(%Request{} = request, opts \\ []) do
    case Repo.transaction(fn -> create_connection(request, opts) end) do
      {:ok, %Result{} = result} ->
        :ok = Manager.track_target(result.target)
        {:ok, result}

      {:error, %Changeset{} = changeset} ->
        {:error, changeset}

      {:error, :internal} ->
        {:error, :internal}
    end
  end

  defp create_connection(request, opts) do
    with :ok <- ensure_unique_target(request),
         {:ok, target} <- create_target(request),
         {:ok, api_key, raw_key} <- generate_key(request.service, opts) do
      %Result{request: request, target: target, api_key: api_key, raw_key: raw_key}
    else
      {:error, %Changeset{} = changeset} ->
        Repo.rollback(changeset)

      {:error, :internal} ->
        Repo.rollback(:internal)
    end
  end

  defp ensure_unique_target(request) do
    case Request.conflict_changeset(request) do
      nil -> :ok
      %Changeset{} = changeset -> {:error, changeset}
    end
  end

  defp create_target(request) do
    attrs =
      request
      |> Request.target_attrs()
      |> Map.put("created_at", DateTime.utc_now() |> DateTime.to_iso8601())

    case %Target{id: ID.target_id()} |> Target.changeset(attrs) |> Repo.insert() do
      {:ok, target} -> {:ok, target}
      {:error, %Changeset{} = changeset} -> {:error, changeset}
    end
  end

  defp generate_key(service, opts) do
    generate_key = Keyword.get(opts, :generate_key, &Auth.generate_key/2)

    case generate_key.("#{service}-ingest", "live") do
      {:ok, api_key, raw_key} -> {:ok, api_key, raw_key}
      {:error, _reason} -> {:error, :internal}
    end
  end
end
