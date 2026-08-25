# l337-audio-server Issues Report

Date: 2026-08-25
System: XIAOMI Redmi Book Pro 16 2024 (Meteor Lake)
Kernel: 7.2.0-1-cachyos

## Summary

The `l337-audio-server` user service is fundamentally misconfigured and fails on every boot. It is **not** the cause of the kernel panic, but it does have multiple architectural and configuration problems that prevent it from functioning.

---

## Issue 1: Duplicate Audio Daemon Spawning

**Severity:** Critical

The service's `ExecStartPre` script (`/opt/l337-audio-server/scripts/start-pipewire.sh`) unconditionally starts a second instance of `pipewire` and `wireplumber` in a separate runtime directory (`/run/l337-audio-server`).

### What happens
- The system already runs `pipewire.service` and `wireplumber.service` as part of the standard desktop audio stack.
- `l337-audio-server.service` declares `After=pipewire.service` and `Wants=pipewire.service`, acknowledging these exist.
- Despite this, `ExecStartPre` tries to launch its own private PipeWire + WirePlumber pair.

### Why this is wrong
1. **Resource conflict** - Two WirePlumber instances fighting over ALSA/PipeWire resources caused the OOM killer storms observed in the previous boot.
2. **Port/socket conflicts** - Both instances attempt to bind similar D-Bus and PipeWire sockets.
3. **Race conditions** - The `sleep 1` in the script is insufficient for a full PipeWire session to initialize, causing the main binary to fail when it tries to connect.

### Evidence
```
ExecStartPre=/opt/l337-audio-server/scripts/start-pipewire.sh
ExecStart=/mnt/private/data/home/tim/git/l337-audio-server/bin/l337-audio-server
After=pipewire.service
Wants=pipewire.service
```

---

## Issue 2: Namespace and Runtime Directory Misconfiguration

**Severity:** Critical

The service uses `RestrictNamespaces=true` but the `ExecStartPre` script attempts to create a private runtime directory (`/run/l337-audio-server`) with specific ownership (`l337:l337`).

### What happens
```
Failed to set up mount namespacing: /run/l337-audio-server: No such file or directory
Failed at step NAMESPACE spawning /opt/l337-audio-server/scripts/start-pipewire.sh: No such file or directory
```

### Why this is wrong
- `RestrictNamespaces=true` prevents the service from creating its own mount namespace.
- The `ExecStartPre` script runs in the same restricted namespace context, so `install -d -m 0700 /run/l337-audio-server` fails because the directory is not pre-created by systemd.
- systemd's `RuntimeDirectory=l337-audio-server` should create `/run/l337-audio-server`, but the `ExecStartPre` script tries to use it before systemd has set it up.

### Evidence
```
[Service]
RestrictNamespaces=true
RuntimeDirectory=l337-audio-server
ExecStartPre=/opt/l337-audio-server/scripts/start-pipewire.sh
```

---

## Issue 3: Wrong User/Group in Script

**Severity:** High

The `start-pipewire.sh` script hardcodes `chown l337:l337`, but the service runs as `tim:tim`.

### Evidence
```
[Service]
User=tim
Group=tim
```
vs
```bash
chown l337:l337 "${XDG_RUNTIME_DIR}"
```

This causes the chown to fail silently (or with errors), leaving `/run/l337-audio-server` owned by `tim:tim`, which then causes PipeWire/WirePlumber started in that directory to fail permission checks.

---

## Issue 4: Hardening Options Too Restrictive for Audio

**Severity:** Medium

The service has many hardening options that are appropriate for network services but conflict with real-time audio requirements.

### Problematic options
```ini
ProtectKernelModules=true
ProtectKernelTunables=true
RestrictNamespaces=true
RestrictRealtime=true
MemoryDenyWriteExecute=false
SystemCallFilter=@system-service
```

### Why these are wrong
- `ProtectKernelModules=true` blocks access to `/dev/dsp`, `/dev/snd/*`, and ALSA devices that the audio server needs.
- `RestrictRealtime=true` prevents the process from requesting real-time scheduling, which is essential for low-latency audio.
- `SystemCallFilter=@system-service` may block `ioctl` calls used by ALSA and PipeWire.

### Evidence
```
[Service]
ProtectKernelModules=true
ProtectKernelTunables=true
RestrictNamespaces=true
RestrictRealtime=true
SystemCallFilter=@system-service
```

---

## Issue 5: ExecStart Points to Wrong Binary Path

**Severity:** High

The service file references:
```
ExecStart=/mnt/private/data/home/tim/git/l337-audio-server/bin/l337-audio-server
```

But the actual binary is installed at:
```
/opt/l337-audio-server/l337-audio-server
```

### Evidence
```
lrwxrwxrwx 1 root root  14728624 Aug 17 21:22 /opt/l337-audio-server/l337-audio-server
```

This means the main binary path in the service file is incorrect. The service would fail even if all other issues were fixed.

---

## Issue 6: Unnecessary Dependencies on System Audio

**Severity:** Medium

The service declares:
```ini
After=pipewire.service
Wants=pipewire.service
After=network-online.target
Wants=network-online.target
```

But the service is supposed to run its own audio server. Depending on the system's PipeWire creates a circular dependency:
- System PipeWire must start before l337-audio-server.
- l337-audio-server tries to start its own PipeWire.
- Result: undefined behavior and resource conflicts.

---

## Issue 7: Missing State Directory Setup

**Severity:** Low

The service declares:
```ini
StateDirectory=l337-audio-server
CacheDirectory=l337-audio-server
ConfigurationDirectory=l337-audio-server
```

But the `ExecStartPre` script does not ensure these directories exist with correct ownership before starting the server. systemd creates them, but if the script or binary expects them earlier, there could be a race.

---

## Recommended Fixes

### Fix 1: Remove Duplicate Audio Daemon Spawning
Remove `ExecStartPre` entirely. Do not start pipewire/wireplumber from within the service.

### Fix 2: Fix Runtime Directory Handling
Either:
- Remove `RestrictNamespaces=true` and let the script manage its own namespace, OR
- Pre-create `/run/l337-audio-server` with correct ownership and permissions, OR
- Use systemd's `RuntimeDirectory` correctly and have the script assume it exists

### Fix 3: Fix User/Group Mismatch
Change the chown in `start-pipewire.sh` from `l337:l337` to `tim:tim`, or remove it entirely if systemd handles it.

### Fix 4: Adjust Hardening for Audio
For an audio server, consider:
```ini
ProtectKernelModules=false
ProtectKernelTunables=false
RestrictNamespaces=false
RestrictRealtime=false
SystemCallFilter=@audio-service + @system-service
```

### Fix 5: Fix ExecStart Binary Path
Change:
```
ExecStart=/mnt/private/data/home/tim/git/l337-audio-server/bin/l337-audio-server
```
To:
```
ExecStart=/opt/l337-audio-server/l337-audio-server
```

### Fix 6: Remove Circular Audio Dependencies
Remove `After=pipewire.service` and `Wants=pipewire.service`. The service should either:
- Use the system's PipeWire (then remove private PipeWire spawning), OR
- Be independent of the system's PipeWire (then remove the After/Wants lines)

### Fix 7: Clean Architecture Decision
Decide on one of these models:

**Model A: System Audio Integration**
- Service uses system PipeWire + WirePlumber
- Remove `ExecStartPre`, remove hardening conflicts
- Service runs as user, accesses existing PipeWire sockets

**Model B: Isolated Audio Server**
- Service runs its own PipeWire + WirePlumber in a sandbox
- Keep `RestrictNamespaces=true` but fix the script to work within systemd's namespace restrictions
- Pre-create runtime directory with correct ownership
- Remove `After=pipewire.service` dependencies

---

## Files Affected
- `/home/tim/.config/systemd/user/l337-audio-server.service`
- `/opt/l337-audio-server/scripts/start-pipewire.sh`
- `/opt/l337-audio-server/scripts/run-server.sh`
- `/opt/l337-audio-server/scripts/install-systemd.sh`
