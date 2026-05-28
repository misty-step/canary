defmodule Canary.Errors.ClassificationTest do
  use ExUnit.Case, async: true

  import ExUnit.CaptureLog

  alias Canary.Errors.Classification

  @unknown %{
    category: :unknown,
    persistence: :unknown,
    component: :unknown
  }

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

    test "keeps specific runtime classes ahead of generic timeout messages" do
      assert Classification.classify(%{
               "error_class" => "FunctionClauseError",
               "message" => "request timed out while pattern matching"
             }) == %{
               category: :application,
               persistence: :persistent,
               component: :runtime
             }
    end

    test "classifies embedding timeouts as transient network infrastructure" do
      assert Classification.classify(%{
               "error_class" => "EmbeddingError",
               "message" => "Embedding request timed out after 20000ms"
             }) == %{
               category: :infrastructure,
               persistence: :transient,
               component: :network
             }
    end

    test "classifies timeout messages as transient network infrastructure" do
      assert Classification.classify(%{
               "error_class" => "Error",
               "message" => "Request timed out while calling upstream"
             }) == %{
               category: :infrastructure,
               persistence: :transient,
               component: :network
             }
    end

    test "classifies transport errors as transient network infrastructure" do
      for error_class <- ["Mint.TransportError", "Req.TransportError"] do
        assert Classification.classify(%{"error_class" => error_class}) == %{
                 category: :infrastructure,
                 persistence: :transient,
                 component: :network
               }
      end
    end

    test "classifies auth/config failures as persistent application runtime failures" do
      assert Classification.classify(%{
               "error_class" => "Error",
               "message" => "CRON_SECRET not configured"
             }) == %{
               category: :application,
               persistence: :persistent,
               component: :runtime
             }
    end

    test "falls back to unknown for unmatched classes" do
      assert Classification.classify(%{"error_class" => "TotallyNewError"}) == @unknown
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

    test "matches custom rules against the error message" do
      rules = [
        %{
          message: ~r/connection reset by peer/,
          classification: %{
            category: :infrastructure,
            persistence: :transient,
            component: :network
          }
        }
      ]

      assert Classification.classify(%{"message" => "connection reset by peer"}, rules) == %{
               category: :infrastructure,
               persistence: :transient,
               component: :network
             }
    end

    test "returns unknown and logs when matcher rules raise for non-map subjects" do
      rules = [
        %{
          message: "not-a-regex",
          classification: %{
            category: :infrastructure,
            persistence: :transient,
            component: :network
          }
        }
      ]

      log =
        capture_log(fn ->
          assert Classification.classify("socket closed", rules) == @unknown
        end)

      assert log =~ "classification_failed"
      assert log =~ "FunctionClauseError"
      assert log =~ ~s(error_class="socket closed")
    end
  end
end
