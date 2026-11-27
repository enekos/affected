# Template for a Homebrew tap (for example enekos/homebrew-tap). Per release, set `version` and
# replace each sha256 with the matching `affected-<target>.tar.gz.sha256` from the GitHub release:
#   gh release download v0.1.0 -R enekos/affected -p '*.sha256' -O - | sort -k2
class Affected < Formula
  desc "Which TypeScript entry points does this diff reach?"
  homepage "https://enekos.github.io/affected/"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/enekos/affected/releases/download/v#{version}/affected-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_aarch64-apple-darwin_SHA256"
    end
    on_intel do
      url "https://github.com/enekos/affected/releases/download/v#{version}/affected-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_x86_64-apple-darwin_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/enekos/affected/releases/download/v#{version}/affected-aarch64-unknown-linux-musl.tar.gz"
      sha256 "REPLACE_WITH_aarch64-unknown-linux-musl_SHA256"
    end
    on_intel do
      url "https://github.com/enekos/affected/releases/download/v#{version}/affected-x86_64-unknown-linux-musl.tar.gz"
      sha256 "REPLACE_WITH_x86_64-unknown-linux-musl_SHA256"
    end
  end

  def install
    bin.install "affected"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/affected --version")
    system "git", "init", "-q"
    (testpath/"index.ts").write "import './a'\n"
    (testpath/"a.ts").write "export {}\n"
    assert_equal "app\n", shell_output("#{bin}/affected -e app=index.ts -c a.ts --names")
  end
end
