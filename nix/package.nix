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

  rev ? "dirty",
}:
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "deepseek-tui";
  version = "git-${rev}";

  src = ../.;

  cargoLock = {
    lockFile = ../Cargo.lock;
  };

  nativeBuildInputs = [
    pkg-config
  ] ++ lib.optionals stdenv.isLinux [
    autoPatchelfHook
  ];

  buildInputs = [
    openssl
  ] ++ lib.optionals stdenv.isLinux [
    dbus.dev
    dbus.lib
    stdenv.cc.cc.lib
  ];

  nativeCheckInputs = [
    python3
    gitMinimal
    cacert
  ];

  cargoBuildFlags = [
    "--package"
    "deepseek-tui-cli"
    "--package"
    "deepseek-tui"
  ];
  cargoTestFlags = finalAttrs.cargoBuildFlags ++ [
    "--lib"
    "--bins"
  ];

  preCheck = ''
    export SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt
  '';

  meta = {
    description = "Coding agent for DeepSeek models that runs in your terminal";
    homepage = "https://github.com/Hmbown/DeepSeek-TUI";
    license = lib.licenses.mit;
    mainProgram = "deepseek";
  };
})
