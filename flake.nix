{
  description = "An over-engineered Hello World in C";

  # Nixpkgs / NixOS version to use.
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let

      # to work with older version of flakes
      lastModifiedDate = self.lastModifiedDate or self.lastModified or "19700101";

      # Generate a user-friendly version number.
      version = builtins.substring 0 8 lastModifiedDate;

      # System types to support.
      supportedSystems = [
        "x86_64-linux"
        "x86_64-darwin"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      # Helper function to generate an attrset '{ x86_64-linux = f "x86_64-linux"; ... }'.
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      # Nixpkgs instantiated for supported system types.
      nixpkgsFor = forAllSystems (
        system:
        import nixpkgs {
          inherit system;
          overlays = [ self.overlay ];
          config = {
            allowUnfree = true;
          };
        }
      );

    in

    {

      # A Nixpkgs overlay.
      overlay =
        final: prev:
        let
          craneLib = crane.mkLib final;
        in
        {
          not-yet = final.callPackage (import ./nix/package.nix) {
            inherit craneLib;
            inherit version;
          };
          not-yet-telegram = final.callPackage (import ./nix/package.nix) {
            inherit craneLib;
            inherit version;
            features = [ "telegram" "daemon" ];
          };
        };

      # Provide some binary packages for selected system types.
      packages = forAllSystems (system: {
        default = nixpkgsFor.${system}.not-yet;
      });

      # A NixOS module, if applicable (e.g. if the package provides a system service).
      nixosModules.cli =
        { ... }:
        {
          nixpkgs.overlays = [ self.overlay ];
        };

      nixosModules.telegram =
        { ... }:
        {
          imports = [ ./nix/module.nix ];
          nixpkgs.overlays = [ self.overlay ];
        };

      # Tests run by 'nix flake check' and by Hydra.
      checks = forAllSystems (system: {
        inherit (self.packages.${system}) default;
      });

    };
}
