{
  lib,
  stdenv,
  autoAddDriverRunpath,
  craneLib,
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

  # Use crane's built-in source cleaning
  promptOrCargo =
    path: type: (builtins.match ".*prompt/.*$" path != null) || (craneLib.filterCargoSources path type);

  cudaBuildInputs = with cudaPackages; [
    cuda_cccl
    cuda_cudart
    libcublas
  ];

  nativeBuildInputs = [
    pkg-config
    cmake
    rustPlatform.bindgenHook
  ]
  ++ lib.optionals cudaSupport [
    cudaPackages.cuda_nvcc
    autoAddDriverRunpath
  ];

  buildInputs = [
    openssl
  ]
  ++ lib.optionals cudaSupport cudaBuildInputs;

  env = lib.optionalAttrs cudaSupport {
    CUDA_COMPUTE_CAP = "89";
    RUSTFLAGS = builtins.concatStringsSep " " [
      "-L ${cudaPackages.cuda_cudart}/lib"
      "-L ${cudaPackages.cuda_cudart}/lib/stubs"
      "-L ${cudaPackages.libcublas.lib}/lib"
      "-L ${cudaPackages.libcublas.static}/lib"
    ];
  };

  # Common args shared between dep-only and full builds
  commonArgs = {
    inherit
      nativeBuildInputs
      buildInputs
      env
      ;
    stdenv = p: effectiveStdenv;

    cargoExtraArgs = lib.concatMapStringsSep " " (f: "--features ${f}") features;
    # Tell crane not to run tests in the build phase
    doCheck = false;
  };

  # Build only dependencies first (allows caching the heavy compile step)
  cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
    src = craneLib.cleanCargoSource ../.;
    version = "0.2.2";
  });

in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    pname = "not-yet";
    src = lib.sources.cleanSourceWith {
      src = ../.;
      filter = promptOrCargo;
      name = "source";
    };
  }
)
