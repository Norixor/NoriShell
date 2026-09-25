# Self-hosted sync: deploy the server and install the plugin

Download `NoriShell_SelfHostSync_Server_<sync-version>.zip` and `NoriShell_SelfHostSync_Plugin_<sync-version>.zip` separately from [NoriShell Releases](https://github.com/Norixor/NoriShell/releases). Both archives share a Sync version, which may differ from the NoriShell app version. The server ZIP contains macOS, Windows, and Linux binaries for x64/ARM64, plus `README.md`. **Do not extract the plugin ZIP**; import it directly into the app. Source is in [`example/self-host-sync`](../../../example/self-host-sync/readme.md).

## 1. Set up the server

1. Extract the server ZIP and select the binary for your server's OS and architecture. For example, on Linux x64, run in a terminal:

   ```sh
   ./norishell-sync-server_linux_x64
   ```

2. On first start, enter a bind port (default `8787`) and enter and confirm a **sync service password** of at least 12 bytes. Input is hidden. By default the server listens only on `127.0.0.1`; configuration and data are stored in the current user's `NoriShell/self-host-sync` configuration directory. Later launches reuse the saved configuration. The server runs in the foreground and stops when the terminal closes or you press Ctrl+C.
3. For a direct connection from another device on your LAN, explicitly run `./norishell-sync-server_linux_x64 --listen 0.0.0.0:8787`. Enter the server's actual IP and port in the plugin, such as `192.168.1.10:8787`; **do not enter `0.0.0.0`**. HTTP transmits the service password, tokens, and request metadata in plaintext. For the Internet or an untrusted network, use an HTTPS reverse proxy and keep the server bound to loopback. An HTTPS IP URL requires a trusted certificate that covers the IP. See the server ZIP's `README.md` for reverse proxy settings, `--data-dir`, background startup, and backups.

Open the server address in a browser (for example `http://127.0.0.1:8787/`) and sign in with the same service password to view read-only status: whether an encrypted exchange exists, its revision, and last write time. The server cannot display the plaintext. Back up the entire server data directory and run only one server process per directory.

### Run the server in the background

On Linux, use systemd. This example places the executable under `/opt/norishell-sync/`, uses `/var/lib/norishell-sync` for data, and runs as `norishell-sync`. `User=norishell-sync` requires that user to **already exist**. For a new installation, create the user and private data directory, then run the binary interactively as that user with **the same `--data-dir`** to set the service password; stop it with Ctrl+C:

```sh
sudo useradd --system --user-group --no-create-home --home-dir /var/lib/norishell-sync --shell /usr/sbin/nologin norishell-sync
sudo install -d -o norishell-sync -g norishell-sync -m 700 /var/lib/norishell-sync
sudo -u norishell-sync /opt/norishell-sync/norishell-sync-server_linux_x64 --data-dir /var/lib/norishell-sync
```

Do not run `useradd` again if the user already exists. For an existing installation, set `--data-dir` to **the directory it already uses**, verify the service user's access to its files, and do not replace existing data with a new empty directory. Stop any manually started process before switching to systemd.

Save the following as `/etc/systemd/system/norishell-sync.service`, adjusting paths and the user:

```ini
[Unit]
Description=NoriShell self-hosted sync
After=network.target

[Service]
Type=simple
User=norishell-sync
ExecStart=/opt/norishell-sync/norishell-sync-server_linux_x64 --data-dir /var/lib/norishell-sync --listen 127.0.0.1:8787
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now norishell-sync
systemctl status norishell-sync
journalctl -u norishell-sync -f
```

If `systemctl status` shows `status=217/USER`, inspect the effective `User=` with `systemctl cat norishell-sync` and check that the user exists with `getent passwd <username>`. systemd has not switched to the service user, so the server binary has not started. Correct the name or create the service user as shown above, then run `sudo systemctl reset-failed norishell-sync` and `sudo systemctl start norishell-sync`. Check data directory permissions and logs afterward.

systemd will start the server at boot and restart it after a failure. Keep `127.0.0.1:8787` when an HTTPS reverse proxy fronts the server. On macOS, use launchd; on Windows, configure Task Scheduler for startup and retry on failure. The current Windows binary does not implement the native Windows Service interface. Initialize the password first and never run two processes against the same data directory. See the [server guide](../../../example/self-host-sync/server/README.md) for more operations detail.

## 2. Import and connect the plugin

1. In NoriShell **Plugins**, import the unextracted `NoriShell_SelfHostSync_Plugin_<sync-version>.zip` and review and grant the page and `sshSync` permissions.
2. Open the new **Sync** page and select “Enter server address,” or use “Settings → Server and plugin settings” on that page. Enter a reachable `http://` or `https://` URL or `IP:port`. A bare `IP:port` uses HTTP; a bare domain uses HTTPS.
3. Select “Connect / Log in” and use the fixed username `owner` and the sync service password from step 1. NoriShell submits the password through its secure form; plugin Wasm cannot read the password or tokens. If login fails, check the address, network, server process, and password.

## 3. Run the first sync

Select “Sync now.” Review the scope, differences, and direction in the protected window before confirming the write. Create or unlock the local Vault on a new device. If remote encrypted data already exists, enter the **Vault password used on the device that first uploaded it** to recover the sync key. The local Vault password may differ. The service password only grants access to the server and cannot replace a Vault password. Updating the plugin ZIP does not require resetting remote data. Background page refresh cannot open the recovery prompt; explicitly select “Sync now.”

## Later use and data scope

Core transfers portable SSH hosts, identities, saved passwords and imported keys, RDP/VNC profiles, and eight groups of non-secret preferences: application, appearance, interaction, highlights, shortcuts, files, desktop, and command notifications. It uploads an independently encrypted exchange, **not the complete Vault file, Vault password, device auto-unlock material, Known Hosts, Agent/FIDO credentials, or external local file paths**.

You can enable automatic sync every 5, 15, 30, or 60 minutes while NoriShell is running, plus a read-only startup check. When both devices change the same item, the default is to use the version with the newer trusted update time; you can choose to be asked instead. Deletions have a separate Ask / Delete automatically setting, with automatic deletion of the newer version as the default. Automatic writes pause when update times are unknown, the baseline is new, Vault needs unlocking, or a restore is unresolved. Device clock differences can affect which version appears newer. Sync stops when the app exits. Preferences restore is checked per group; failures and conflicts remain available for review. If SSH or desktop restore was not confirmed, keep local preferences, clear the pending item, and start a new manual sync.

When sync deletes a host, its saved forwarding rules are deleted in the same transaction under the same deletion setting. When confirmation is required, the prompt lists the affected rules. Deleting saved rules does not stop running forwarding sessions.

Changing the service password revokes active sessions. See the [server guide](../../../example/self-host-sync/server/README.md) for operations, backups, and API details.
