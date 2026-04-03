{
  lib,
  pkgs,
  stdenv,
  autoAddDriverRunpath,

  fetchFromGitHub,
  rustPlatform,
  cudaPackages,
  features ? [ ],
  pkg-config,
  openssl,
  cmake,
  ...
}:
let
  cudaSupport = lib.elem "cuda" features;
  effectiveStdenv = if cudaSupport then cudaPackages.backendStdenv else stdenv;
  cudaBuildInputs = with cudaPackages; [
    cuda_cccl # <nv/target>

    # A temporary hack for reducing the closure size, remove once cudaPackages
    # have stopped using lndir: https://github.com/NixOS/nixpkgs/issues/271792
    cuda_cudart
    libcublas
  ];
in
rustPlatform.buildRustPackage.override { stdenv = effectiveStdenv; } rec {
  pname = "not-yet";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    filter =
      name: type:
      let
        noneOf = builtins.all (x: !x);
        baseName = baseNameOf name;
      in
      noneOf [
        (lib.hasSuffix ".nix" name) # Ignore *.nix files when computing outPaths
        (lib.hasSuffix ".md" name) # Ignore *.md changes whe computing outPaths
        (lib.hasPrefix "." baseName) # Skip hidden files and directories
        (baseName == "flake.lock")
      ];
    src = lib.cleanSource ../.;
  };

  nativeBuildInputs = [
    pkg-config
    cmake
  ]
  ++ lib.optionals cudaSupport [
    cudaPackages.cuda_nvcc
    autoAddDriverRunpath
  ];

  buildInputs = [
    rustPlatform.bindgenHook
    openssl
  ]
  ++ lib.optionals cudaSupport cudaBuildInputs;

  cargoLock = {
    lockFile = "${src}/Cargo.lock";
    allowBuiltinFetchGit = true;
  };

  buildFeatures = features;

  env = lib.optionalAttrs cudaSupport {
    CUDA_COMPUTE_CAP = "89";
    RUSTFLAGS = builtins.concatStringsSep " " [
      "-L ${cudaPackages.cuda_cudart}/lib"
      "-L ${cudaPackages.cuda_cudart}/lib/stubs"
      "-L ${cudaPackages.libcublas.lib}/lib"
      "-L ${cudaPackages.libcublas.static}/lib"
    ];
  };
}
