<div align="center">
  <img src="assets/icon/prime-files.png" alt="" width="112" height="112">

  # Prime Files

  Explorador de arquivos para Windows, rápido, escrito em Rust.
</div>

Barra de título com abas, miniaturas e ícones do próprio Windows, menu do botão direito do Explorador, visualizador de quase tudo e um modo compacto no estilo do Zen Browser.

## Baixar

Pegue o instalador na página de [Releases](https://github.com/CakeDev4k/prime-files/releases/latest) (`PrimeFiles-Setup-<versão>.exe`) e execute. A instalação é só para o seu usuário e não pede administrador. O instalador baixa o que faltar:

- o **Visual C++ Runtime** da Microsoft, se o computador não tiver;
- o **7-Zip**, se você marcar a opção, para ver `.rar`, `.7z` e `.iso` no visualizador;
- no Windows edição "N", o **Media Feature Pack**, sem o qual o programa não abre.

Depois de instalado, o Prime Files se atualiza sozinho: procura uma versão nova ao abrir, baixa em segundo plano e instala quando você fecha o programa (ou na hora, em Configurações › Sobre). Dá para desligar em Configurações › Sobre.

O instalador não é assinado digitalmente: o Windows pode mostrar "O Windows protegeu o computador". Clique em **Mais informações → Executar assim mesmo**. Cada release traz um `SHA256SUMS.txt` para conferir o arquivo.

Requer Windows 10 (1903) ou mais novo, 64 bits.

## Linux / Debian

Existe uma porta do Prime Files para Linux/Debian, feita em Rust e com a mesma interface. O pacote Debian instala o executável com o nome do programa, `prime-files`, mesmo que dentro do código ele ainda se chame `nohrs` (os crates são `nohrs-*`).

Três formas de instalar:

**Pacote `.deb` (sistema inteiro).** O repositório tem os arquivos do `debhelper` em `debian/`:

```bash
dpkg-buildpackage -b -us -uc
sudo apt install ../prime-files_0.3.9-1_amd64.deb
```

**Script de usuário (sem root).** Instala em `~/.local`, para o seu usuário:

```bash
cargo build --release -p nohrs
./packaging/install-linux.sh
```

O mesmo script com `--uninstall` remove o que ele instalou.

**Cargo:**

```bash
cargo install --path crates/nohrs
```

O `.deb` também traz o ícone (`/usr/share/icons/hicolor/256x256/apps/prime-files.png`) e a entrada do menu (`/usr/share/applications/prime-files.desktop`).

Dependências de execução: `libxkbcommon0`, `libwayland-client0`, `libwayland-cursor0`, `libwayland-egl1`, `libx11-6`, `libxcb1`, `libvulkan1`, `libfontconfig1` e `fonts-dejavu-core` (a fonte do terminal). `ffmpeg` (prévias de vídeo) e `poppler-utils` (prévias de PDF, pelo `pdftoppm`) são recomendados.

No Linux ainda não existem estas partes, que são do Windows:

- o menu do botão direito do Explorador (no Linux o menu oferece os programas que o ambiente gráfico registra para aquele tipo de arquivo);
- o índice NTFS no estilo do Everything (a busca varre as pastas, em vez de ler o índice do disco);
- o plugin do Listary;
- as miniaturas do Shell do Windows (no Linux o visualizador usa o cache de miniaturas do freedesktop e o tema de ícones do ambiente gráfico);
- a atualização automática dentro do programa (o instalador do Windows é quem se atualiza; no Linux a versão vem pelo pacote);
- o atalho global de busca disparado por Ctrl duas vezes (o atalho gravado pelo usuário funciona pelo X11).

O resto funciona igual: abas, divisão em dois painéis, visualizador, terminal embutido, busca, atalhos e o modo compacto. Registrar o Prime Files como o programa que abre pastas também funciona, do jeito do desktop (pelo `mimeapps.list`).

Mais detalhes em [`docs/build-and-display-linux.md`](docs/build-and-display-linux.md).

## O que tem

- **Abas** na barra de título, divisão em dois painéis, abas restauradas ao reabrir
- **Vistas** Detalhes, Lista, Árvore e Ícones, com zoom contínuo (Ctrl + roda) e a ordem e o zoom lembrados por pasta
- **Visualizador** de imagens, GIF, vídeo e áudio (com forma de onda), PDF, modelos 3D (OBJ, STL, glTF, FBX), texto, Markdown, pastas e arquivos compactados
- **Integração com o Windows**: menu do botão direito do Explorador, arrastar e soltar com outros programas, miniaturas do Shell, "mostrar na pasta" de outros programas abrindo aqui
- **Busca em todo o computador** no estilo do Everything (lupa da barra de baixo ou Ctrl+E), nas subpastas (Ctrl+F) e filtro rápido (é só digitar na lista)
- **Ouvir sons passando o mouse** sobre arquivos de áudio, com o tempo de espera ajustável
- **Armazenamento** de cada unidade em Este Computador e na barra lateral
- **Terminal** embutido com abas (Ctrl+')
- **Doze idiomas**: português, inglês, espanhol, catalão, francês, alemão, italiano, húngaro, russo, japonês, árabe e chinês simplificado
- **Modo compacto** no estilo do Zen Browser: abas, barra de rota, barra lateral, visualizador e barra de baixo escondidos até o mouse encostar na borda, por cima da lista ou empurrando-a, numa moldura de cantos arredondados
- **Explorador padrão** do Windows, opcional (Configurações › Integração com o Windows): pastas, drives e Win+E abrem no Prime Files
- **Listary**, opcional (Configurações › Integração com o Windows): pastas abertas pelo Listary abrem no Prime Files; o desinstalador devolve ao Explorador
- **Atualização automática** pelas releases deste repositório

## Compilar

Precisa do [Rust](https://rustup.rs) (stable) e do Windows.

```bash
cargo build --release -p nohrs
```

No Linux, o build precisa também dos pacotes de desenvolvimento do Vulkan, do Wayland e do X11 (o gpui liga neles):

```bash
sudo apt install libvulkan-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev
```

O programa fica em `target/release/nohrs.exe` (o código usa internamente o nome `nohrs`: crates `nohrs-*` e a pasta de configuração). O instalador sai de `installer/prime-files.iss` com o [Inno Setup 6](https://jrsoftware.org/isinfo.php); o workflow `.github/workflows/release.yml` faz os dois e publica a release a cada tag `v*`. O ícone é gerado por `assets/icon/make_icon.py`.

Inclui uma cópia do [gpui](https://github.com/zed-industries/zed) 0.2.2 com correções para o Windows, descritas em [`third_party/gpui/NOHRS.md`](third_party/gpui/NOHRS.md).

## Licença

MIT, veja [`LICENSE`](LICENSE).
