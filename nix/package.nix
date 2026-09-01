{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  autoPatchelfHook ? null,
  openssl,
  dbus ? null,

  # for cargo test
  python3,
  gitMinimal,
  cacert,
  procps,

  rev ? "dirty",
}:
let
  # Shared libraries the check-phase test binaries need at runtime
  # (libdbus, libgcc_s). Derived once so LD_LIBRARY_PATH and the patchelf
  # RPATH can never drift apart.
  runtimeLibraryPath = lib.makeLibraryPath (
    lib.optionals stdenv.hostPlatform.isLinux [
      dbus.lib
      stdenv.cc.cc.lib
    ]
  );
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "codewhale";
  version = "git-${rev}";

  src = ../.;

  cargoLock = {
    lockFile = ../Cargo.lock;
  };

  nativeBuildInputs = [
    pkg-config
  ] ++ lib.optionals stdenv.hostPlatform.isLinux [
    autoPatchelfHook
  ];

  buildInputs = [
    openssl
  ] ++ lib.optionals stdenv.hostPlatform.isLinux [
    dbus.dev
    dbus.lib
    stdenv.cc.cc.lib
  ];

  nativeCheckInputs = [
    python3
    gitMinimal
    cacert
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [
    # fleet host memory/zombie sampling shells out to `ps`; NixOS has no
    # system-wide /usr/bin/ps, so make it resolvable through PATH.
    procps
  ];

  cargoBuildFlags = [
    "--package"
    "codewhale-cli"
  ];  # single binary — tui crate is a library, not a shipped binary (v0.9.5+)
  cargoTestFlags = finalAttrs.cargoBuildFlags ++ [
    "--lib"
    "--bins"
    "--"
    # Requires the checkout itself to be a git repository (the test walks up
    # from the current dir); the Nix source tree has no .git.
    "--skip"
    "tools::subagent::tests::git_repo_root_reports_attempted_paths_when_no_repo_found"
    # The header width table is calibrated against the git chrome label
    # populated by the surrounding checkout state, which the sandboxed test
    # process does not see.
    "--skip"
    "tui::underwater::tests::configured_session_tokens_follow_underwater_header_width_priority"
  ];

  preCheck = ''
    # Tests write to the default config/home locations; the sandbox HOME
    # (/homeless-shelter) is not writable.
    export HOME="$(mktemp -d)"
    export SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt
  ''
  + lib.optionalString stdenv.hostPlatform.isLinux ''
    # nixpkgs no longer derives LD_LIBRARY_PATH from buildInputs; the cargo
    # test binaries link libdbus/libgcc_s dynamically and have no RPATH until
    # autoPatchelfHook runs at fixup (after the check phase).
    export LD_LIBRARY_PATH=${runtimeLibraryPath}
  '';

  # Two-stage check: build the harnesses first, give them an explicit RPATH,
  # then run. Fleet and shell tests re-execute the test binary through a
  # scrubbed environment (no LD_LIBRARY_PATH), so without this the re-spawned
  # harness cannot load libdbus and every descendant-tree test times out.
  checkPhase = ''
    runHook preCheck

    # Tests mutate process-global environment (PATH/HOME/cwd) via the shared
    # EnvVarGuard; a concurrent test spawning a shell can then fail to resolve
    # the interpreter. Run the harness serially so the check is deterministic.
    export RUST_TEST_THREADS=1

    flagsArray=(-j "$NIX_BUILD_CORES" --profile release --target ${stdenv.hostPlatform.config} --offline)
    concatTo flagsArray cargoTestFlags checkFlags

    echo "Building test binaries"
    cargo test --no-run "''${flagsArray[@]}"

    ${lib.optionalString stdenv.hostPlatform.isLinux ''
      echo "Patching runtime RPATH into test binaries"
      find "target/${stdenv.hostPlatform.config}/release/deps" \
        -maxdepth 1 -type f -executable \
        -exec patchelf --add-rpath "${runtimeLibraryPath}" {} +
    ''}

    echo "Running tests"
    cargo test "''${flagsArray[@]}"

    runHook postCheck
  '';

  meta = {
    description = "Terminal coding agent for DeepSeek";
    homepage = "https://github.com/Hmbown/CodeWhale";
    license = lib.licenses.mit;
    mainProgram = "codewhale";
  };
})
