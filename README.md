# Not Yet

Use LLM for notification filtering.

## Features

- RSS Feed
- Telegram Bot
- CLI Tool

## Installing

### Cargo Install

If cargo is available on your system, installation can be done by running:

```bash
# Use feature 'metal' for Macs
cargo install --git https://github.com/zhufucdev/not-yet.git not_yet --features cuda

# Also avaiable as telegram bot
cargo install --git https://github.com/zhufucdev/not-yet.git not_yet --features 'cuda, telegram'
```

### Nix Flake

Those running nixOS or nix-darwin can install by using flakes:
```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    not-yet.url = "github:zhufucdev/not-yet";
  };
  outputs =
    inputs@{
      self,
      nixpkgs,
      sops-nix,
      not-yet,
      ...
    }:
    {
      nixosConfigurations.functionaltux = nixpkgs.lib.nixosSystem {
        modules = [
          ./configuration.nix
          not-yet.nixosModules.cli
          # Also avaiable as telegram bot
          not-yet.nixosModules.telegram
        ];
      };
    };
}
```
