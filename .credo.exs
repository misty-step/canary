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
      requires: [],
      checks: %{
        enabled: [
          # Disable AliasUsage — inline qualified calls are clearer in controllers
          # where the alias would be used once or twice.
          {Credo.Check.Design.AliasUsage, false}
        ]
      }
    }
  ]
}
