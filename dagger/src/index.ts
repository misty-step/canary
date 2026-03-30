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

/** Elixir container with system deps, hex/rebar, and caches mounted. */
function elixirContainer(
  source: Directory,
  mixEnv: string,
  buildCacheName: string,
): Container {
  const depsCache: CacheVolume = dag.cacheVolume("elixir-deps")
  const buildCache: CacheVolume = dag.cacheVolume(buildCacheName)
  const hexCache: CacheVolume = dag.cacheVolume("hex-home")

  return dag
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
    .withEnvVariable("MIX_ENV", mixEnv)
    .withMountedCache("/app/deps", depsCache)
    .withMountedCache("/app/_build", buildCache)
    .withMountedCache("/root/.hex", hexCache)
    .withMountedDirectory("/app", source)
    .withWorkdir("/app")
    .withExec(["mix", "deps.get"])
}

@object()
export class Ci {
  /**
   * Base container: Elixir + deps fetched + compiled in test env.
   * Shared by every check so we only compile once per source change.
   */
  @func()
  base(source: Directory): Container {
    return elixirContainer(source, "test", "elixir-build-test").withExec([
      "mix",
      "compile",
      "--warnings-as-errors",
    ])
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

  /** mix sobelow --config --exit */
  @func()
  async sobelow(source: Directory): Promise<string> {
    return this.base(source)
      .withExec(["mix", "sobelow", "--config", "--exit", "--threshold", "medium"])
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

  /** mix dialyzer (runs in dev env, separate build cache) */
  @func()
  async dialyzer(source: Directory): Promise<string> {
    const pltCache: CacheVolume = dag.cacheVolume("dialyzer-plt")

    return elixirContainer(source, "dev", "elixir-build-dev")
      .withMountedCache("/app/_build/dev", pltCache)
      .withExec(["mix", "dialyzer"])
      .stdout()
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
        (await container
          .withExec(["mix", "sobelow", "--config", "--exit", "--threshold", "medium"])
          .stdout())
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
