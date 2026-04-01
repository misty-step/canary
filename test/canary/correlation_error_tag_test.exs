defmodule Canary.CorrelationErrorTagTest do
  use ExUnit.Case, async: true

  alias Canary.CorrelationErrorTag

  describe "format/1" do
    test "formats exception tuple with module atom" do
      assert CorrelationErrorTag.format({:exception, Ecto.InvalidChangesetError}) ==
               "Ecto.InvalidChangesetError"
    end

    test "formats nested kind/reason tuple" do
      assert CorrelationErrorTag.format({:exit, :normal}) == "exit:normal"
    end

    test "formats bare atom" do
      assert CorrelationErrorTag.format(:timeout) == "timeout"
    end

    test "formats struct by module name" do
      assert CorrelationErrorTag.format(%RuntimeError{message: "boom"}) ==
               "RuntimeError"
    end

    test "formats unknown term as unexpected" do
      assert CorrelationErrorTag.format("something weird") == "unexpected"
    end
  end
end
