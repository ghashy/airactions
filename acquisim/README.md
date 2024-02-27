# acquisim

This is a bank simulator that stores its data on the RAM.
There is also [acqui](https://github.com/ghashy/acqui), written in Swift for macOS, which serves as an Acquisim management client.

> [!IMPORTANT]
> Currently, `acquisim` supports a single-store account.

Acquisim was designed to be simple. It can create/delete accounts, open credits, create transactions, track balances, bank emission. With a simple internal design, it aims to offer real-life API interaction, just like in real acquiring services.

## Usage:

You can either build the Docker container yourself in this directory or use a pre-built image from Docker Hub:
```bash
docker pull ghashy/acquisim
```

You need to pass a configuration file and a secret file as secrets. For example, using docker-compose:
```yaml
services:
  acquisim:
    image: ghashy/acquisim:0.1
    expose:
      - "15100"
    secrets:
      - terminal-password
      - example-config
    environment:
      TERMINAL_PASSWORD_FILE: /run/secrets/terminal-password
      ACQUISIM_CONFIG_FILE: /run/secrets/example-config
secrets:
  example-config:
    file: secrets/example_config.yaml
  terminal-password:
    file: secrets/terminal_password.txt
```

After running, use [acqui](https://github.com/ghashy/acqui) for bank management and [acquisim-api](https://github.com/ghashy/acquirust/tree/main/backends/acquisim-api) for store-bank interaction.