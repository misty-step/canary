defmodule Canary.ServiceOnboarding do
  @moduledoc """
  Opinionated onboarding flow for connecting one service to Canary.
  """

  alias Canary.ServiceOnboarding.{Connect, Payload, Request}
  alias Ecto.Changeset

  @type connect_error :: {:validation, Changeset.t()} | :internal

  @spec connect(map(), String.t(), keyword()) :: {:ok, map()} | {:error, connect_error()}
  def connect(params, base_url, opts \\ []) do
    with {:ok, request} <- Request.apply(params),
         {:ok, result} <- Connect.connect(request, opts) do
      {:ok, Payload.render(result, base_url)}
    else
      {:error, %Changeset{} = changeset} ->
        {:error, {:validation, changeset}}

      {:error, :internal} ->
        {:error, :internal}
    end
  end
end
