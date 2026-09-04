#!/bin/bash
set -Ceuo pipefail
bin="$1"
tests=(
    shm_bgra_baseline
    shm_bgra_ssse3
    shm_bgra_avx2
    shm_bgra_avx512bw
    shm_bgra_avx512vbmi
    gpu
)
shm_bgra_baseline () {
    env=( MULTIBG_DEBUG_CPU=baseline )
    multibg
    assert_screenshot_eq
}
shm_bgra_ssse3 () {
    env=( MULTIBG_DEBUG_CPU=ssse3 )
    multibg
    assert_screenshot_eq
}
shm_bgra_avx2 () {
    env=( MULTIBG_DEBUG_CPU=avx2 )
    multibg
    assert_screenshot_eq
}
shm_bgra_avx512bw () {
    env=( MULTIBG_DEBUG_CPU=avx512bw )
    multibg
    assert_screenshot_eq
}
shm_bgra_avx512vbmi () {
    env=( MULTIBG_DEBUG_CPU=avx512vbmi )
    multibg
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
        RUST_BACKTRACE=1 \
        "${env[@]}" "$bin" --log-level=trace "$@" w \
        >>multibg.log 2>&1 &
    multibg_pid=$!
    sleep 0.5
    env=()
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
sleep 0.5
wld="$(ls -t $XDG_RUNTIME_DIR | grep -m 1 '^wayland-[[:digit:]]$')"
sock="$XDG_RUNTIME_DIR/sway-ipc.$UID.$sway_pid.sock"
swaymsg '[title = "wlroots - WL-1"] floating enable'
sleep 0.2
swaymsg '[title = "wlroots - WL-1"] resize set 551 391'
sleep 0.2
passed=0
set +e
for tst in "${tests[@]}"; do
    "$tst"
    status=$?
    kill $multibg_pid
    wait $multibg_pid
    sleep 0.2
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
