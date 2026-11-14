#!/usr/bin/env bash
# Up to 1.3.5 the client was called "Mnema Desktop DEV". This checks that such
# a release upgrades in place to the packages of this build:
#
#   .deb      the same package (mnema-desktop-dev) is upgraded: one package,
#             the old folder under /opt gone, the command, the menu entry and
#             the AppArmor profile pointing at the new folder.
#   AppImage  the client updates an AppImage by replacing the file where it
#             lies (src/core/appimage.ts), so the new program must start under
#             the old file name.
#
# Run from linux/ after packaging, under the conditions of the smoke test
# (no other copy installed); needs gh (GH_TOKEN), sudo, xvfb-run and xwininfo.
#
#   bash scripts/upgrade-from-dev.sh <old version> <deb arch> <AppImage arch>
set -euo pipefail
[ "$#" = 3 ] || { echo 'usage: upgrade-from-dev.sh <old version> <deb arch> <AppImage arch>' >&2; exit 2; }
from="$1"
deb_arch="$2"
appimage_arch="$3"
fail() { echo "::error::$*"; exit 1; }

old="Mnema-Desktop-DEV_${from}"
old_deb="${old}_${deb_arch}.deb"
old_appimage="${old}_${appimage_arch}.AppImage"
dir="$(mktemp -d)"
trap 'rm -rf -- "$dir"' EXIT
gh release download "v$from" --repo msch128/mnema-talk-desktop --dir "$dir" \
  --pattern "$old_deb" --pattern "$old_appimage" --pattern SHA256SUMS
(cd "$dir" && grep -E "  (${old_deb}|${old_appimage})\$" SHA256SUMS | sha256sum -c -)

new_version="$(node -p "require('./package.json').version")"
new_deb="$(ls release/*.deb)"
new_appimage="$(ls release/*.AppImage)"
case "$new_deb$new_appimage" in *DEV*) fail "this build is still named DEV: $new_deb $new_appimage" ;; esac

# .deb: the old release, installed and started, then this build over it.
sudo apt-get install -y "$dir/$old_deb"
SMOKE_TITLE='Mnema Desktop DEV' xvfb-run -a bash scripts/smoke.sh "/opt/Mnema Desktop DEV/mnema-desktop-dev"
# dpkg also replaces a package of the same version, which a pull request's build has.
sudo dpkg -i "$new_deb"
installed="$(dpkg-query -W -f='${db:Status-Abbrev}${Package}\n' 'mnema-desktop*' | grep -c '^ii')"
[ "$installed" = 1 ] || fail "$installed packages named mnema-desktop* are installed, expected one"
[ "$(dpkg-query -W -f='${Version}' mnema-desktop-dev)" = "$new_version" ] || fail 'mnema-desktop-dev was not upgraded'
[ ! -e '/opt/Mnema Desktop DEV' ] || fail 'the folder of the old version is left in /opt'
[ "$(readlink -f /usr/bin/mnema-desktop-dev)" = '/opt/Mnema Desktop/mnema-desktop-dev' ] || fail 'the command does not start the new folder'
entry=/usr/share/applications/mnema-desktop-dev.desktop
grep -qx 'Name=Mnema Desktop' "$entry" || fail "the menu entry is not named Mnema Desktop: $(grep '^Name=' "$entry")"
grep -q '^Exec="*/opt/Mnema Desktop/mnema-desktop-dev' "$entry" || fail "the menu entry starts $(grep '^Exec=' "$entry")"
if [ -f /etc/apparmor.d/mnema-desktop-dev ]; then
  grep -qF '/opt/Mnema Desktop/' /etc/apparmor.d/mnema-desktop-dev || fail 'the AppArmor profile does not cover the new folder'
  ! grep -qF 'Mnema Desktop DEV' /etc/apparmor.d/mnema-desktop-dev || fail 'the AppArmor profile still names the old folder'
fi
xvfb-run -a bash scripts/smoke.sh '/opt/Mnema Desktop/mnema-desktop-dev'
echo "ok: $old_deb upgraded in place to $(basename "$new_deb")"

# AppImage: the old one where a person keeps it, then replaced the way the client's update does.
apps="$dir/apps"
mkdir "$apps"
install -m 0755 "$dir/$old_appimage" "$apps/$old_appimage"
SMOKE_TITLE='Mnema Desktop DEV' xvfb-run -a bash scripts/smoke.sh env APPIMAGE_EXTRACT_AND_RUN=1 "$apps/$old_appimage"
install -m 0755 "$new_appimage" "$apps/$old_appimage.new"
mv -f "$apps/$old_appimage.new" "$apps/$old_appimage"
xvfb-run -a bash scripts/smoke.sh env APPIMAGE_EXTRACT_AND_RUN=1 "$apps/$old_appimage"
echo "ok: $old_appimage replaced by $(basename "$new_appimage") starts under its old name"
