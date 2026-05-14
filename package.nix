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
  manifest = builtins.fromTOML (builtins.readFile ./zigzag/Cargo.toml);
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
  pname = "zigzag";
  version = manifest.workspace.package.version;

  src = lib.cleanSource ./zigzag;
  cargoLock.lockFile = ./zigzag/Cargo.lock;

  cargoBuildFlags = [
    "--package"
    "zigzag-cli"
    "--bin"
    "zigzag"
  ];
  cargoTestFlags = [ "--workspace" ];

  nativeBuildInputs = [ makeWrapper ];
  nativeCheckInputs = [ git ];

  postInstall = ''
    wrapProgram "$out/bin/zigzag" \
      --prefix PATH : ${lib.makeBinPath runtimeDependencies}
  '';

  passthru.runtimeDependencies = runtimeDependencies;

  meta = {
    description = "Zigzag TUI/CLI project manager for Zellij-based development";
    homepage = "https://github.com/arkan/zigzag";
    mainProgram = "zigzag";
    platforms = lib.platforms.unix;
  };
}
