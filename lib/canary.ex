defmodule Canary do
  @moduledoc """
  Canary keeps the contexts that define your domain
  and business logic.

  Contexts are also responsible for managing your data, regardless
  if it comes from the database, an external API or others.
  """

  @doc "Read-only repo. Routes to Repo in test for sandbox compatibility."
  def read_repo, do: Application.get_env(:canary, :read_repo, Canary.ReadRepo)
end
