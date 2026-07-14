#!/bin/bash
set -Ceuo pipefail
multibg_bin="$1"
tests=(
    shm_rgb
    shm_bgra
    gpu
)
shm_rgb () {
    multibg
    assert_screenshot_eq
}
shm_bgra () {
    multibg --pixelformat baseline
    assert_screenshot_eq
}
gpu () {
    multibg --gpu
    assert_screenshot_eq
}

multibg () {
    env -i \
        XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
        I3SOCK="$sock" SWAYSOCK="$sock" \
        WAYLAND_DISPLAY="$wld" \
        RUST_BACKTRACE=1 RUST_LOG=info,multibg_wayland=trace \
        "$multibg_bin" "$@" w \
        >>multibg.log 2>&1 &
    multibg_pid=$!
    sleep 0.5
}
screenshot () {
    env -i \
        XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
        WAYLAND_DISPLAY="$wld" \
        grim -l 1 "$1"
}
assert_image_eq () {
    local out="$(magick compare -metric AE "$1" "$2" 'null:' 2>&1)"
    [[ "$out" == '0 (0)' ]]
}
assert_screenshot_eq () {
    screenshot screenshot.png
    assert_image_eq w/WL-1/_default.png screenshot.png
}

if [[ -e /tmp/multibg-test ]]; then
    rm -r /tmp/multibg-test
fi
mkdir /tmp/multibg-test
cd /tmp/multibg-test
echo swaybg_command /bin/true >> sway.config
echo bar swaybar_command /bin/true >> sway.config
mkdir -p w/WL-1
# 551x391 ( 19 * 29 x 17 * 23 )
magick /usr/share/backgrounds/sway/Sway_Wallpaper_Blue_1920x1080.png \
    -gravity Center -crop 551x391-0-0 \
    w/WL-1/_default.png
env -i \
    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
    WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
    sway --config sway.config \
    >/dev/null 2>&1 &
sway_pid=$!
sleep 0.2
wld="$(ls -t $XDG_RUNTIME_DIR | grep -m 1 '^wayland-[[:digit:]]$')"
sock="$XDG_RUNTIME_DIR/sway-ipc.$UID.$sway_pid.sock"
swaymsg '[title = "wlroots - WL-1"] floating enable'
sleep 0.1
swaymsg '[title = "wlroots - WL-1"] resize set 551 391'
sleep 0.1
passed=0
set +e
for tst in "${tests[@]}"; do
    "$tst"
    status=$?
    kill $multibg_pid
    wait $multibg_pid
    sleep 0.1
    if (( status == 0 )); then
        passed=$(( passed + 1 ))
    else
        echo FAILED "$tst"
        passed=-1
        break
    fi
done
kill $sway_pid
if (( passed >= 0 )); then
    echo PASSED "$passed" tests
    rm -r /tmp/multibg-test
    exit 0
else
    exit 1
fi
