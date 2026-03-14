defmodule Canary.Repo do
  use Ecto.Repo,
    otp_app: :canary,
    adapter: Ecto.Adapters.SQLite3
end
