# Stand-in for `cargo` while Gradle packages the `.so` files Nix built.
#
# The istmo Gradle plugin runs `cargo build --manifest-path <m> --target
# <triple> … --message-format=json…` and copies the cdylib named by the
# `compiler-artifact` message. This answers with the prebuilt library
# under `$NOTINPLUS_JNI_DIR/<abi>/` instead of compiling.

manifest=""
target=""
command="${1:-}"
shift || true
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest-path) manifest="$2"; shift 2 ;;
    --target) target="$2"; shift 2 ;;
    *) shift ;;
  esac
done

if [ "$command" != build ]; then
  echo "cargo shim: \`cargo $command\` is not available; the Rust side is prebuilt by Nix (nix/android.nix)" >&2
  exit 1
fi

case "$target" in
  aarch64-linux-android) abi=arm64-v8a ;;
  x86_64-linux-android) abi=x86_64 ;;
  armv7-linux-androideabi) abi=armeabi-v7a ;;
  i686-linux-android) abi=x86 ;;
  *) echo "cargo shim: unknown target '$target'" >&2; exit 1 ;;
esac

lib=$(find -L "${NOTINPLUS_JNI_DIR:?set NOTINPLUS_JNI_DIR to the jniLibs lib/ dir}/$abi" -maxdepth 1 -name '*.so' -print -quit)
if [ -z "$lib" ]; then
  echo "cargo shim: no prebuilt library for $abi under $NOTINPLUS_JNI_DIR" >&2
  exit 1
fi

printf '{"reason":"compiler-artifact","manifest_path":"%s","target":{"kind":["cdylib"]},"filenames":["%s"]}\n' \
  "$manifest" "$lib"
