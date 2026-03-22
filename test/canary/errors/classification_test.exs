defmodule Canary.Errors.ClassificationTest do
  use ExUnit.Case, async: true

  alias Canary.Errors.Classification

  describe "classify/1" do
    test "classifies database connection errors as transient infrastructure" do
      assert Classification.classify(%{"error_class" => "DBConnection.ConnectionError"}) == %{
               category: :infrastructure,
               persistence: :transient,
               component: :database
             }
    end

    test "classifies runtime clause errors as persistent application failures" do
      assert Classification.classify(%{"error_class" => "FunctionClauseError"}) == %{
               category: :application,
               persistence: :persistent,
               component: :runtime
             }
    end

    test "falls back to unknown for unmatched classes" do
      assert Classification.classify(%{"error_class" => "TotallyNewError"}) == %{
               category: :unknown,
               persistence: :unknown,
               component: :unknown
             }
    end

    test "applies custom table rules without matcher changes" do
      rules = [
        %{
          error_class: ~r/(^|\.)Mint\.TransportError$/,
          classification: %{
            category: :infrastructure,
            persistence: :transient,
            component: :network
          }
        }
      ]

      assert Classification.classify(%{"error_class" => "Mint.TransportError"}, rules) == %{
               category: :infrastructure,
               persistence: :transient,
               component: :network
             }
    end
  end
end
