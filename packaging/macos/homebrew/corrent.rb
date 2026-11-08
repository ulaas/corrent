# Homebrew cask template. The package-manifests workflow fills in the version and SHA-256
# (the dmg) after a release is published. Until the dmg is signed and notarized, use it from a
# personal tap (`brew tap ulaas/tap`); homebrew/cask only accepts apps that pass Gatekeeper.
cask "corrent" do
  version "{{VERSION}}"
  sha256 "{{SHA256}}"

  url "https://github.com/ulaas/corrent/releases/download/v#{version}/Corrent-#{version}-macos.dmg"
  name "Corrent"
  desc "BitTorrent client with an egui interface and the librqbit engine"
  homepage "https://ulaas.github.io/corrent/"

  livecheck do
    url :url
    strategy :github_latest
  end

  app "Corrent.app"

  zap trash: [
    "~/Library/Application Support/Corrent",
    "~/Library/Preferences/io.github.ulaas.Corrent.plist",
    "~/Library/Saved Application State/io.github.ulaas.Corrent.savedState",
  ]
end
