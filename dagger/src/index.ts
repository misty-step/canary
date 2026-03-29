/**
 * CI pipeline for Canary — mirrors .github/workflows/ci.yml
 *
 * Run the full quality gate:  dagger call all --source=.
 * Run a single check:         dagger call test --source=.
 */
import {
  dag,
  Container,
  Directory,
  CacheVolume,
  object,
  func,
} from "@dagger.io/dagger"

const ELIXIR_IMAGE =
  "hexpm/elixir:1.17.3-erlang-27.3-debian-bookworm-20250224-slim"

@object()
export class Ci {
  /**
   * Base container: Elixir + deps fetched + compiled in test env.
   * Shared by every check so we only compile once per source change.
   */
  @func()
  base(source: Directory): Container {
    const depsCache: CacheVolume = dag.cacheVolume("elixir-deps")
    const buildCache: CacheVolume = dag.cacheVolume("elixir-build-test")
    const hexCache: CacheVolume = dag.cacheVolume("hex-home")

    return (
      dag
        .container()
        .from(ELIXIR_IMAGE)
        // system deps for SQLite NIF compilation
        .withExec(["apt-get", "update", "-q"])
        .withExec([
          "apt-get",
          "install",
          "-yq",
          "--no-install-recommends",
          "build-essential",
          "git",
        ])
        .withExec(["apt-get", "clean"])
        .withExec(["mix", "local.hex", "--force"])
        .withExec(["mix", "local.rebar", "--force"])
        .withEnvVariable("MIX_ENV", "test")
        .withMountedCache("/app/deps", depsCache)
        .withMountedCache("/app/_build", buildCache)
        .withMountedCache("/root/.hex", hexCache)
        .withMountedDirectory("/app", source)
        .withWorkdir("/app")
        .withExec(["mix", "deps.get"])
        .withExec(["mix", "compile", "--warnings-as-errors"])
    )
  }

  /** mix format --check-formatted */
  @func()
  async format(source: Directory): Promise<string> {
    return this.base(source)
      .withExec(["mix", "format", "--check-formatted"])
      .stdout()
  }

  /** mix credo --strict */
  @func()
  async credo(source: Directory): Promise<string> {
    return this.base(source)
      .withExec(["mix", "credo", "--strict"])
      .stdout()
  }

  /** mix sobelow --config */
  @func()
  async sobelow(source: Directory): Promise<string> {
    return this.base(source)
      .withExec(["mix", "sobelow", "--config"])
      .stdout()
  }

  /** mix deps.audit */
  @func()
  async audit(source: Directory): Promise<string> {
    return this.base(source)
      .withExec(["mix", "deps.audit"])
      .stdout()
  }

  /** MIX_ENV=test mix test --cover */
  @func()
  async test(source: Directory): Promise<string> {
    return (
      this.base(source)
        .withExec(["mix", "ecto.create", "--quiet"])
        .withExec(["mix", "ecto.migrate", "--quiet"])
        .withExec(["mix", "test", "--cover"])
        .stdout()
    )
  }

  /** mix dialyzer (runs in dev env, separate caches) */
  @func()
  async dialyzer(source: Directory): Promise<string> {
    const depsCache: CacheVolume = dag.cacheVolume("elixir-deps")
    const buildCache: CacheVolume = dag.cacheVolume("elixir-build-dev")
    const hexCache: CacheVolume = dag.cacheVolume("hex-home")
    const pltCache: CacheVolume = dag.cacheVolume("dialyzer-plt")

    return (
      dag
        .container()
        .from(ELIXIR_IMAGE)
        .withExec(["apt-get", "update", "-q"])
        .withExec([
          "apt-get",
          "install",
          "-yq",
          "--no-install-recommends",
          "build-essential",
          "git",
        ])
        .withExec(["apt-get", "clean"])
        .withExec(["mix", "local.hex", "--force"])
        .withExec(["mix", "local.rebar", "--force"])
        .withEnvVariable("MIX_ENV", "dev")
        .withMountedCache("/app/deps", depsCache)
        .withMountedCache("/app/_build", buildCache)
        .withMountedCache("/root/.hex", hexCache)
        .withMountedCache("/app/_build/dev/dialyxir_plt", pltCache)
        .withMountedDirectory("/app", source)
        .withWorkdir("/app")
        .withExec(["mix", "deps.get"])
        .withExec(["mix", "dialyzer"])
        .stdout()
    )
  }

  /**
   * Run the full quality gate: compile, format, credo, sobelow, audit, test.
   * Dialyzer excluded by default (slow PLT build on first run).
   */
  @func()
  async all(source: Directory): Promise<string> {
    const container = this.base(source)

    // Run each check sequentially off the shared base.
    // SQLite single-writer means we can't safely parallelize DB-touching steps.
    const results: string[] = []

    results.push(
      "=== format ===\n" +
        (await container
          .withExec(["mix", "format", "--check-formatted"])
          .stdout())
    )

    results.push(
      "=== credo ===\n" +
        (await container.withExec(["mix", "credo", "--strict"]).stdout())
    )

    results.push(
      "=== sobelow ===\n" +
        (await container.withExec(["mix", "sobelow", "--config"]).stdout())
    )

    results.push(
      "=== audit ===\n" +
        (await container.withExec(["mix", "deps.audit"]).stdout())
    )

    results.push(
      "=== test ===\n" +
        (await container
          .withExec(["mix", "ecto.create", "--quiet"])
          .withExec(["mix", "ecto.migrate", "--quiet"])
          .withExec(["mix", "test", "--cover"])
          .stdout())
    )

    return results.join("\n")
  }
}
