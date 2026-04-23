%{
  configs: [
    %{
      name: "default",
      strict: true,
      files: %{
        included: ["lib/", "test/"],
        excluded: [~r"/_build/", ~r"/deps/"]
      },
      plugins: [],
      requires: [
        "./test/support/credo_checks/ecto_pk_via_cast.ex",
        "./test/support/credo_checks/preload_then_take.ex"
      ],
      checks: %{
        enabled: [
          {Canary.Checks.EctoPKViaCast, []},
          {Canary.Checks.PreloadThenTake, []},
          # Disable AliasUsage — inline qualified calls are clearer in controllers
          # where the alias would be used once or twice.
          {Credo.Check.Design.AliasUsage, false},
          {Credo.Check.Refactor.CyclomaticComplexity, [max_complexity: 12]},
          {Credo.Check.Refactor.Nesting, [max_nesting: 3]}
        ]
      }
    }
  ]
}
