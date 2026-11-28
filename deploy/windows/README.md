# DentalClarity on Windows

This folder builds the Windows installer for clinical mode: the `dentalclarity.exe` service on the practice network, HTTPS on the LAN, and Bedrock and Polly in the practice's own AWS account. DentalClarity is a prototype. Do not use it with real patient records until the [clinical-readiness gaps](../../docs/SECURITY.md#clinical-readiness-gap-checklist) are closed.

| File | Purpose |
|---|---|
| `dentalclarity.iss` | Inno Setup 6 script (docs/DECISIONS.md E26) |
| `dentalclarity.env.example` | Settings template, installed once as `%ProgramData%\DentalClarity\dentalclarity.env` |

## Build the installer

Needs Rust, Trunk, the `wasm32-unknown-unknown` target and [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`ISCC.exe`). From the repository root:

```powershell
cd crates\dc-web; trunk build --release; cd ..\..
cargo build --release -p dc-server --features aws
& "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" deploy\windows\dentalclarity.iss
```

The installer is written to `target\installer\DentalClarity-0.1.0-setup.exe`. It is not code-signed, so SmartScreen warns on first run.

## What the installer does

1. Copies `dentalclarity.exe`, the knowledge base and the synthetic practice to `C:\Program Files\DentalClarity`.
2. Writes the settings template to `%ProgramData%\DentalClarity\dentalclarity.env`, once. Upgrades and uninstalls keep it.
3. Runs `dentalclarity.exe service install`, which registers the `DentalClarity` service (automatic start) under the virtual account `NT SERVICE\DentalClarity`.
4. Restricts `%ProgramData%\DentalClarity` to SYSTEM, Administrators and the service account. DPAPI machine scope lets any local process unwrap stored secrets, so this ACL is what keeps other accounts out.
5. Adds the inbound firewall rule `DentalClarity HTTPS` for TCP 8443 from the local subnet only.
6. Leaves the service stopped, so the settings can be edited first.

Uninstalling stops and removes the service and the firewall rule and keeps `%ProgramData%\DentalClarity`.

## First start

1. Edit `%ProgramData%\DentalClarity\dentalclarity.env`. Set `DC_PUBLIC_ORIGIN` to the address the tablets will use, for example `https://dentalclarity.practice.lan:8443` or `https://192.168.1.20:8443`. Give the server a fixed address or DNS name first.
2. To use AWS, store the access key in an elevated prompt and uncomment `DC_AI_PROVIDER=aws`:

   ```powershell
   & "C:\Program Files\DentalClarity\dentalclarity.exe" secret set aws-access-key-id
   & "C:\Program Files\DentalClarity\dentalclarity.exe" secret set aws-secret-access-key
   ```

   Each value is read from the console and stored DPAPI-protected under `%ProgramData%\DentalClarity\data\secrets`. `secret delete NAME` removes one.
3. Start the service: `sc.exe start DentalClarity`.
4. The one-time setup code is in `%ProgramData%\DentalClarity\data\setup-code.txt`. Open the app from a workstation, enter the code and create the staff account. The file is removed at the next start after the account exists.
5. The log is `%ProgramData%\DentalClarity\logs\dentalclarity.log`.

## Trust the certificate on every device

With `DC_TLS=local-ca` the server creates its own certificate authority on first start and exports it to `%ProgramData%\DentalClarity\data\tls\dentalclarity-ca.crt` (`dentalclarity.exe tls ca-path` prints the path). Install that file once on each device that opens DentalClarity. Only the certificate is copied; its private key never leaves the server.

| Device | Steps |
|---|---|
| Windows workstation | Elevated prompt: `certutil -addstore Root dentalclarity-ca.crt`. Edge and Chrome use the Windows store. Firefox needs `security.enterprise_roots.enabled` set to true, or an import under Settings, Privacy & Security, Certificates. |
| iPad or iPhone | Send the file by AirDrop or email and open it, install the profile under Settings, General, VPN & Device Management, then turn on full trust under Settings, General, About, Certificate Trust Settings. |
| Android tablet | Settings, Security, Encryption & credentials, Install a certificate, CA certificate. The exact menu names vary by maker. |
| macOS | Open the file in Keychain Access, add it to the System keychain and set it to Always Trust. |

The server certificate is reissued from this CA on every start, so it never needs renewal by hand. The CA itself is valid for ten years. If the CA key is lost, a new CA is created and every device must trust the new file. Enable `DC_HSTS` only after every device trusts the certificate.

Windows tools built on Schannel, such as the bundled `curl.exe`, fail on this certificate with `CERT_TRUST_REVOCATION_STATUS_UNKNOWN`, because a local CA publishes no revocation list. Browsers do not fail on this. For `curl.exe`, add `--ssl-no-revoke`.

A certificate from the practice's IT or internal CA is the better choice where one is available: set `DC_TLS_CERT` and `DC_TLS_KEY` to PEM files and remove `DC_TLS`.

## AWS permissions

Give the access key an IAM user or role with only these permissions, in the configured region:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "bedrock:InvokeModel",
      "Resource": [
        "arn:aws:bedrock:us-west-2::foundation-model/anthropic.claude-haiku-5-5*",
        "arn:aws:bedrock:us-west-2:ACCOUNT_ID:inference-profile/*anthropic.claude-haiku-5-5*"
      ]
    },
    { "Effect": "Allow", "Action": "polly:SynthesizeSpeech", "Resource": "*" }
  ]
}
```

The Converse API is authorized by `bedrock:InvokeModel`. Model access must be enabled for the account in the Bedrock console. A long-lived access key needs rotation; short-lived credentials (for example IAM Roles Anywhere) are an open item in docs/SECURITY.md.

## Without the installer

```powershell
dentalclarity.exe service install
sc.exe start DentalClarity
dentalclarity.exe service uninstall
```

`service install` must run in an elevated prompt from the folder the service should run from. Set the data folder ACL and firewall rule by hand as listed above.
