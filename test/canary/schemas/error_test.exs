defmodule Canary.Schemas.ErrorTest do
  use Canary.DataCase, async: true

  alias Canary.Schemas.Error

  test "accepts known classification domains" do
    changeset =
      Error.changeset(%Error{}, %{
        service: "svc",
        error_class: "RuntimeError",
        message: "boom",
        group_hash: "group-hash",
        created_at: "2026-03-24T00:00:00Z",
        classification_category: "infrastructure",
        classification_persistence: "transient",
        classification_component: "database"
      })

    assert changeset.valid?
  end

  test "rejects invalid classification domains" do
    changeset =
      Error.changeset(%Error{}, %{
        service: "svc",
        error_class: "RuntimeError",
        message: "boom",
        group_hash: "group-hash",
        created_at: "2026-03-24T00:00:00Z",
        classification_category: "oops",
        classification_persistence: "forever",
        classification_component: "kernel"
      })

    refute changeset.valid?

    assert errors_on(changeset) == %{
             classification_category: ["is invalid"],
             classification_component: ["is invalid"],
             classification_persistence: ["is invalid"]
           }
  end
end
