{
  lib,
  rustPlatform,
  makeWrapper,
  zellij,
  worktrunk,
  git,
  gh,
  openssh,
  mosh,
}:

let
  manifest = builtins.fromTOML (builtins.readFile ./z/Cargo.toml);
  runtimeDependencies = [
    zellij
    worktrunk
    git
    gh
    openssh
    mosh
  ];
in
rustPlatform.buildRustPackage {
  pname = "z";
  version = manifest.workspace.package.version;

  src = lib.cleanSource ./z;
  cargoLock.lockFile = ./z/Cargo.lock;

  cargoBuildFlags = [
    "--package"
    "z-cli"
    "--bin"
    "z"
  ];
  cargoTestFlags = [ "--workspace" ];

  nativeBuildInputs = [ makeWrapper ];
  nativeCheckInputs = [ git ];

  postInstall = ''
    wrapProgram "$out/bin/z" \
      --prefix PATH : ${lib.makeBinPath runtimeDependencies}
  '';

  passthru.runtimeDependencies = runtimeDependencies;

  meta = {
    description = "TUI/CLI project manager for Zellij-based development";
    homepage = "https://github.com/arkan/z";
    mainProgram = "z";
    platforms = lib.platforms.unix;
  };
}
