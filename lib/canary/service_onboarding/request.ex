defmodule Canary.ServiceOnboarding.Request do
  use Ecto.Schema

  import Ecto.Changeset
  import Ecto.Query

  alias Canary.Health.SSRFGuard
  alias Canary.Repos
  alias Canary.Schemas.Target

  @primary_key false
  embedded_schema do
    field :service, :string
    field :url, :string
    field :environment, :string, default: "production"
    field :interval_ms, :integer
    field :allow_private, :boolean, default: false
  end

  @type t :: %__MODULE__{
          service: String.t() | nil,
          url: String.t() | nil,
          environment: String.t() | nil,
          interval_ms: integer() | nil,
          allow_private: boolean() | nil
        }
  @service_conflict_message "already has a health target"
  @url_conflict_message "is already monitored"

  @spec apply(map()) :: {:ok, t()} | {:error, Ecto.Changeset.t()}
  def apply(attrs) do
    attrs
    |> changeset()
    |> Ecto.Changeset.apply_action(:insert)
  end

  @spec changeset(map()) :: Ecto.Changeset.t()
  def changeset(attrs) do
    %__MODULE__{}
    |> cast(attrs, [:service, :url, :environment, :interval_ms, :allow_private])
    |> trim(:service)
    |> trim(:url)
    |> trim(:environment)
    |> validate_required([:service, :url])
    |> validate_number(:interval_ms, greater_than: 0)
    |> validate_url()
    |> validate_uniqueness()
  end

  @spec target_attrs(t()) :: map()
  def target_attrs(%__MODULE__{} = request) do
    %{
      "name" => request.service,
      "service" => request.service,
      "url" => request.url
    }
    |> maybe_put("interval_ms", request.interval_ms)
  end

  @spec conflict_changeset(t()) :: Ecto.Changeset.t() | nil
  def conflict_changeset(%__MODULE__{} = request) do
    case conflict_errors(request) do
      [] ->
        nil

      errors ->
        Enum.reduce(errors, change(request), fn {field, message}, changeset ->
          add_error(changeset, field, message)
        end)
    end
  end

  defp trim(changeset, field) do
    update_change(changeset, field, &String.trim/1)
  end

  defp validate_url(changeset) do
    case {get_field(changeset, :url), allow_private?(changeset)} do
      {url, allow_private} when is_binary(url) and url != "" ->
        case SSRFGuard.validate_url(url, allow_private) do
          :ok -> changeset
          {:error, reason} -> add_error(changeset, :url, reason)
        end

      _ ->
        changeset
    end
  end

  defp validate_uniqueness(changeset) do
    case uniqueness_request(changeset) do
      %__MODULE__{} = request -> merge_conflicts(changeset, conflict_errors(request))
      nil -> changeset
    end
  end

  defp conflict?(query) do
    not is_nil(Repos.read_repo().one(query))
  end

  defp conflict_errors(%__MODULE__{service: service, url: url}) do
    []
    |> maybe_append_conflict(
      :service,
      service,
      service_conflict_query(service),
      @service_conflict_message
    )
    |> maybe_append_conflict(:url, url, url_conflict_query(url), @url_conflict_message)
  end

  defp maybe_append_conflict(errors, _field, value, _query, _message)
       when not is_binary(value) or value == "" do
    errors
  end

  defp maybe_append_conflict(errors, field, _value, query, message) do
    if conflict?(query) do
      [{field, message} | errors]
    else
      errors
    end
  end

  defp merge_conflicts(changeset, errors) do
    Enum.reduce(Enum.reverse(errors), changeset, fn {field, message}, acc ->
      add_error(acc, field, message)
    end)
  end

  defp uniqueness_request(changeset) do
    case {get_field(changeset, :service), get_field(changeset, :url)} do
      {service, url} when is_binary(service) and service != "" and is_binary(url) and url != "" ->
        %__MODULE__{service: service, url: url}

      _ ->
        nil
    end
  end

  defp service_conflict_query(service) do
    from(target in Target,
      where: target.service == ^service,
      select: target.id,
      limit: 1
    )
  end

  defp url_conflict_query(url) do
    from(target in Target,
      where: target.url == ^url,
      select: target.id,
      limit: 1
    )
  end

  defp allow_private?(changeset) do
    get_field(changeset, :allow_private) or
      Application.get_env(:canary, :allow_private_targets, false)
  end

  defp maybe_put(map, _key, nil), do: map
  defp maybe_put(map, key, value), do: Map.put(map, key, value)
end
