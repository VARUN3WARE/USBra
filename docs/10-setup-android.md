# USBra — Android Client Setup (deliverable #11)

## 1. Prepare the phone (once)

1. Settings → About phone → tap **Build number** 7× (enables Developer options).
2. Settings → System → Developer options → enable **USB debugging**.
3. Plug into the PC over USB; unlock the phone; accept the
   **"Allow USB debugging?"** RSA dialog (check "always allow").

No root. No app-side network permissions beyond localhost `INTERNET`
(declared in the manifest for socket use).

## 2. Build the APK

### Option A — Android Studio (recommended)

1. Android Studio → **Open** → select the `android/` directory.
2. Let Gradle sync (it generates the wrapper; first sync downloads
   AGP 8.5.2 + Kotlin 2.0.20 — needs network once).
3. **Run ▶** with the phone connected — Studio installs and launches it.

Requires: SDK platform 34 (Studio prompts to install), JDK 17 (bundled).

### Option B — command line

```bash
# once: install a JDK 17 and Gradle ≥ 8.7 (or use Android Studio's)
sudo apt install -y openjdk-17-jdk
cd android
gradle wrapper --gradle-version 8.9     # generates ./gradlew
./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## 3. Run

1. PC: `scripts/adb-usb-setup.sh` (terminal 1) and `scripts/run-demo.sh`
   (terminal 2) — see `docs/09-setup-ubuntu.md`.
2. Phone: open **USBra** from the launcher.
3. Expect: "connected" within ~1 s, then the animated test pattern with a
   stats overlay (fps / RTT ms / MB/s / frame count / format).

The overlay format: `fps 60 | rtt 2.1ms | 3.4MB/s | f 1234 | BGRX 1600x900`.

## 4. What to verify by hand (M3–M5 acceptance)

- Unplug USB → overlay flips to "reconnecting…"; replug → picture returns
  (RECONNECT path, host resyncs with a full frame).
- Red box = pure red, not blue (pixel-format check).
- Smooth 60 fps motion of the box/bar; no accumulating lag (bounded queue).

## 5. Troubleshooting

| Symptom | Fix |
|---|---|
| App opens, stays "connecting" | `adb reverse --list` must show the tunnel; host must be listening (`ss -ltn | grep 8899`) |
| Builds fail on JDK | Use JDK 17 (JDK 25 is too new for AGP 8.5) |
| Gradle sync needs SDK 34 | Accept the Studio prompt or `sdkmanager "platforms;android-34"` |
| App killed in background | Keep it foregrounded while using the display (M9 adds a foreground service) |
| Battery drain when idle | None expected — damage-driven: 0 frames when the virtual display is static |
