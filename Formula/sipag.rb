class Sipag < Formula
  desc "Unattended GitHub issue worker — sleep while Claude writes your PRs"
  homepage "https://github.com/dorky-robot/sipag"
  url "https://github.com/dorky-robot/sipag.git", branch: "main"
  version "0.2.0"
  license "MIT"

  depends_on "bash"       # bash 4+ (macOS ships 3.x)
  depends_on "jq"
  depends_on "gh"
  depends_on "rust" => :build # for TUI

  def install
    # Bash scripts → libexec
    libexec.install "bin/sipag"
    (libexec/"lib").install Dir["lib/*"]

    # Wrapper that sets SIPAG_ROOT and delegates
    (bin/"sipag").write_env_script(libexec/"sipag",
      SIPAG_ROOT: libexec.to_s)

    # Build and install TUI
    cd "tui" do
      system "cargo", "install", *std_cargo_args
    end
  end

  test do
    assert_match "sipag v", shell_output("#{bin}/sipag version")
  end
end
