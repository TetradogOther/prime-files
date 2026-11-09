#!/bin/sh
# Instala o Prime Files no Linux para o seu próprio usuário, sem root.
#
# O script instala o programa, o ícone e a entrada do menu, e o --uninstall
# remove exatamente esses três arquivos (nada mais).
set -e

PREFIX="${HOME}/.local"
BINARY=""
UNINSTALL="no"

usage() {
    cat <<'EOF'
Uso: install-linux.sh [opções] [caminho-do-binário]

Instala o Prime Files para o seu usuário, sem precisar de root.

Opções:
  --uninstall          remove o que este script instalou
  --binary CAMINHO     usa este executável em vez de procurar um
  --prefix DIRETÓRIO   instala em outro lugar (padrão: ~/.local)
  -h, --help           mostra esta ajuda

Arquivos instalados:
  <prefix>/bin/prime-files
  <prefix>/share/icons/hicolor/256x256/apps/prime-files.png
  <prefix>/share/applications/prime-files.desktop
EOF
}

die() {
    echo "install-linux.sh: $*" >&2
    exit 1
}

# Where this script lives: the icon and the desktop entry are next to it.
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

while [ $# -gt 0 ]; do
    case "$1" in
        --uninstall)
            UNINSTALL="yes"
            shift
            ;;
        --binary)
            [ $# -ge 2 ] || die "--binary precisa de um caminho"
            BINARY="$2"
            shift 2
            ;;
        --binary=*)
            BINARY="${1#--binary=}"
            shift
            ;;
        --prefix)
            [ $# -ge 2 ] || die "--prefix precisa de um diretório"
            PREFIX="$2"
            shift 2
            ;;
        --prefix=*)
            PREFIX="${1#--prefix=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        -*)
            die "opção desconhecida: $1"
            ;;
        *)
            break
            ;;
    esac
done

# A path given after the options is the program to install.
if [ $# -gt 0 ] && [ -z "$BINARY" ]; then
    BINARY="$1"
    shift
fi
[ $# -eq 0 ] || die "sobra argumento: $1"

PROGRAM="$PREFIX/bin/prime-files"
ICON="$PREFIX/share/icons/hicolor/256x256/apps/prime-files.png"
DESKTOP="$PREFIX/share/applications/prime-files.desktop"

# The desktop's caches are only read when something asks for them.
refresh_caches() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q "$PREFIX/share/applications" || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" || true
    fi
}

# The path of the program goes into the desktop entry, so it is written with
# the characters that mean something to sed escaped.
escape_sed() {
    printf '%s' "$1" | sed -e 's/[\\&|]/\\&/g'
}

# The desktop entry names the program by its full path: the desktop does not
# always read the PATH of the person who is logged in.
write_desktop() {
    escaped=$(escape_sed "$PROGRAM")
    sed -e "s|^Exec=prime-files %f$|Exec=$escaped %f|" \
        -e "s|^Exec=prime-files$|Exec=$escaped|" \
        "$SCRIPT_DIR/prime-files.desktop" > "$DESKTOP"
}

if [ "$UNINSTALL" = "yes" ]; then
    removed="no"
    for file in "$PROGRAM" "$ICON" "$DESKTOP"; do
        if [ -e "$file" ] || [ -L "$file" ]; then
            rm -f "$file"
            echo "Removido: $file"
            removed="yes"
        fi
    done
    # The folders are only removed when they end up empty.
    rmdir "$PREFIX/share/icons/hicolor/256x256/apps" 2>/dev/null || true
    rmdir "$PREFIX/share/icons/hicolor/256x256" 2>/dev/null || true
    rmdir "$PREFIX/share/icons/hicolor" 2>/dev/null || true
    rmdir "$PREFIX/share/icons" 2>/dev/null || true
    rmdir "$PREFIX/share/applications" 2>/dev/null || true
    refresh_caches
    if [ "$removed" = "yes" ]; then
        echo "Prime Files removido. O resto do sistema não foi tocado."
    else
        echo "Nada do Prime Files estava instalado em $PREFIX."
    fi
    exit 0
fi

# The program to install: the one given, or the one this tree built.
if [ -z "$BINARY" ]; then
    for candidate in \
        "$SCRIPT_DIR/../target/release/nohrs" \
        "$SCRIPT_DIR/../target/debug/nohrs" \
        "$SCRIPT_DIR/../../target/release/nohrs"
    do
        if [ -f "$candidate" ]; then
            BINARY="$candidate"
            break
        fi
    done
fi
if [ -z "$BINARY" ]; then
    BINARY=$(command -v nohrs 2>/dev/null || true)
fi
[ -n "$BINARY" ] || die "não achei o executável; compile com 'cargo build --release -p nohrs' ou use --binary"
[ -f "$BINARY" ] || die "não achei o executável: $BINARY"

[ -f "$SCRIPT_DIR/prime-files.png" ] || die "falta o ícone: $SCRIPT_DIR/prime-files.png"
[ -f "$SCRIPT_DIR/prime-files.desktop" ] || die "falta a entrada do menu: $SCRIPT_DIR/prime-files.desktop"

mkdir -p "$PREFIX/bin" \
         "$PREFIX/share/icons/hicolor/256x256/apps" \
         "$PREFIX/share/applications"

cp -f "$BINARY" "$PROGRAM"
chmod 755 "$PROGRAM"
cp -f "$SCRIPT_DIR/prime-files.png" "$ICON"
chmod 644 "$ICON"
write_desktop
chmod 644 "$DESKTOP"

refresh_caches

echo "Prime Files instalado."
echo
echo "  Programa:  $PROGRAM"
echo "  Ícone:     $ICON"
echo "  Menu:      $DESKTOP"
echo
case ":${PATH:-}:" in
    *":$PREFIX/bin:"*)
        echo "Para abrir, rode:"
        echo
        echo "    prime-files"
        ;;
    *)
        echo "Para abrir, rode o caminho completo:"
        echo
        echo "    $PROGRAM"
        echo
        echo "O diretório $PREFIX/bin não está no PATH. Para poder escrever só"
        echo "'prime-files', acrescente esta linha ao seu ~/.bashrc (ou ~/.zshrc):"
        echo
        echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac
echo
echo "Para desfazer, rode de novo este script com --uninstall."
