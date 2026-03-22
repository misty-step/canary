defmodule Canary.Errors.Classification do
  @moduledoc """
  Deterministic, table-driven error classifier.
  """

  @type classification :: %{
          category: :infrastructure | :application | :unknown,
          persistence: :transient | :persistent | :unknown,
          component: :database | :network | :runtime | :unknown
        }

  @type fields :: %{
          error_class: String.t(),
          message: String.t()
        }

  @type rule :: %{
          required(:classification) => classification(),
          optional(:error_class) => Regex.t(),
          optional(:message) => Regex.t()
        }

  @unknown %{
    category: :unknown,
    persistence: :unknown,
    component: :unknown
  }

  @rules [
    %{
      error_class: ~r/(^|\.)DBConnection\.ConnectionError$/,
      classification: %{
        category: :infrastructure,
        persistence: :transient,
        component: :database
      }
    },
    %{
      error_class: ~r/(^|\.)FunctionClauseError$/,
      classification: %{
        category: :application,
        persistence: :persistent,
        component: :runtime
      }
    }
  ]

  @spec classify(map() | struct()) :: classification()
  @spec classify(map() | struct(), [rule()]) :: classification()
  def classify(subject, rules \\ @rules) when is_list(rules) do
    fields = normalize(subject)

    Enum.find_value(rules, @unknown, fn rule ->
      if matches?(fields, rule), do: rule.classification
    end)
  rescue
    _ -> @unknown
  end

  defp normalize(subject) when is_map(subject) do
    %{
      error_class: value(subject, :error_class),
      message: value(subject, :message)
    }
  end

  defp normalize(_), do: %{error_class: "", message: ""}

  defp matches?(fields, rule) do
    Enum.all?([:error_class, :message], fn key ->
      case Map.get(rule, key) do
        nil -> true
        pattern -> Regex.match?(pattern, Map.fetch!(fields, key))
      end
    end)
  end

  defp value(subject, key) do
    Map.get(subject, key) || Map.get(subject, Atom.to_string(key)) || ""
  end
end
