# The template for Casks/tectr.rb in johannes-krabbe/homebrew-tap. scripts/release.sh
# fills in the version and checksum, so edit this file rather than the tap's copy.
cask "tectr" do
  version "@VERSION@"
  sha256 "@SHA256@"

  url "https://github.com/Johannes-Krabbe/tectr/releases/download/v#{version}/tectr_#{version}_darwin_arm64.tar.gz"
  name "tectr"
  desc "Run, watch and steer many coding agents in parallel"
  homepage "https://github.com/Johannes-Krabbe/tectr"

  depends_on arch: :arm64
  depends_on :macos

  app "tectr.app"
  binary "tectr"
  binary "tectr-daemon"

  postflight do
    # The app isn't notarized, so Gatekeeper would refuse to open it.
    system_command "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "#{appdir}/tectr.app"]
    system_command "/usr/bin/xattr",
                   args: ["-d", "com.apple.quarantine", "#{staged_path}/tectr", "#{staged_path}/tectr-daemon"]

    # The daemon runs as a LaunchAgent: it starts at login, restarts if it crashes, and
    # outlives the app, so closing the window doesn't stop agents. --shell-env gives it
    # the login shell's PATH, which launchd doesn't.
    label = "dev.tectr.daemon"
    domain = "gui/#{Process.uid}"
    plist = Pathname("~/Library/LaunchAgents/#{label}.plist").expand_path
    logs = Pathname("~/Library/Logs/tectr").expand_path
    plist.dirname.mkpath
    logs.mkpath
    plist.write <<~XML
      <?xml version="1.0" encoding="UTF-8"?>
      <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
      <plist version="1.0">
      <dict>
        <key>Label</key>
        <string>#{label}</string>
        <key>ProgramArguments</key>
        <array>
          <string>#{HOMEBREW_PREFIX}/bin/tectr-daemon</string>
          <string>--shell-env</string>
        </array>
        <key>RunAtLoad</key>
        <true/>
        <key>KeepAlive</key>
        <true/>
        <key>ProcessType</key>
        <string>Interactive</string>
        <key>StandardOutPath</key>
        <string>#{logs}/daemon.log</string>
        <key>StandardErrorPath</key>
        <string>#{logs}/daemon.log</string>
      </dict>
      </plist>
    XML

    # Restart onto the new binary. Stopping waits for the daemon to stop its agents.
    system_command "/bin/launchctl", args: ["bootout", "#{domain}/#{label}"]
    40.times do
      break unless system("/bin/launchctl", "print", "#{domain}/#{label}", out: File::NULL, err: File::NULL)

      sleep 0.5
    end
    system_command "/bin/launchctl", args: ["bootstrap", domain, plist.to_s]
  end

  uninstall launchctl: "dev.tectr.daemon"

  zap trash: [
    "~/.config/tectr",
    "~/Library/Application Support/tectr",
    "~/Library/Caches/dev.tectr.dsk",
    "~/Library/Logs/tectr",
    "~/Library/WebKit/dev.tectr.dsk",
  ]
end
