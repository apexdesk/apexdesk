# ApexDesk

**English** · [Español](#español)

ApexDesk is a light, fast office suite: documents, spreadsheets,
presentations, PDF, databases and notebooks in one program. It opens and
saves Microsoft Office (.docx, .xlsx, .pptx) and LibreOffice (.odt, .ods,
.odp) files.

- No account, no cloud, no telemetry, no AI. It works offline.
- One small program that starts instantly.
- Windows and Linux.

This repository holds only the installers. They are under
**[Releases](../../releases/latest)**.

## Downloads

| System | File |
|---|---|
| Windows 10/11 (64-bit) | `apexdesk-<version>-windows-x86_64.zip`: unzip and run `ApexDesk.exe` |
| Windows, portable (USB) | `apexdesk-<version>-windows-x86_64-portable.zip`: keeps its settings next to the program |
| Debian, Ubuntu, Mint | `apexdesk_<version>_amd64.deb`: `sudo apt install ./apexdesk_*.deb` |
| Fedora, openSUSE | `apexdesk-<version>-1.x86_64.rpm`: `sudo dnf install ./apexdesk-*.rpm` |
| Arch Linux, Manjaro | `apexdesk-bin-<version>-1-x86_64.pkg.tar.zst`: `sudo pacman -U ./apexdesk-bin-*.pkg.tar.zst` |
| Linux, portable (USB) | `apexdesk-<version>-linux-x86_64-portable.tar.gz`: unpack and run `./apexdesk` |

Linux needs glibc 2.31 or newer (Ubuntu 20.04, Debian 11, Fedora 32 and later).

Or install it straight from the release:

```sh
sudo pacman -U https://github.com/apexdesk/apexdesk/releases/download/v0.8.0/apexdesk-bin-0.8.0-1-x86_64.pkg.tar.zst
```

The package is built from [`aur/apexdesk-bin`](aur/apexdesk-bin); to build it yourself with makepkg:

```sh
git clone https://github.com/apexdesk/apexdesk.git
cd apexdesk/aur/apexdesk-bin && makepkg -si
```

### Windows SmartScreen

The program isn't signed yet, so Windows may say "Windows protected your
PC". Click **More info**, then **Run anyway**.

## License

ApexDesk is free, but not open source. The terms are in the
[license agreement](EULA.md):

- **Free with no time limit** for individuals, the self-employed,
  companies under 10 employees, education, small municipalities,
  volunteer associations and religious denominations.
- **Free until 31 December 2027** for other companies and organizations.

The open-source components it uses are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

---

## Español

ApexDesk es una suite ofimática ligera y rápida: documentos, hojas de
cálculo, presentaciones, PDF, bases de datos y cuadernos de notas en un
solo programa. Abre y guarda archivos de Microsoft Office (.docx, .xlsx,
.pptx) y de LibreOffice (.odt, .ods, .odp).

- Sin cuenta, sin nube, sin telemetría y sin IA. Funciona sin conexión.
- Un solo programa pequeño que arranca al instante.
- Windows y Linux.

Este repositorio solo contiene los instaladores. Están en
**[Releases](../../releases/latest)**.

| Sistema | Archivo |
|---|---|
| Windows 10/11 (64 bits) | `apexdesk-<versión>-windows-x86_64.zip`: descomprime y ejecuta `ApexDesk.exe` |
| Windows, portable (USB) | `apexdesk-<versión>-windows-x86_64-portable.zip`: guarda sus ajustes junto al programa |
| Debian, Ubuntu, Mint | `apexdesk_<versión>_amd64.deb`: `sudo apt install ./apexdesk_*.deb` |
| Fedora, openSUSE | `apexdesk-<versión>-1.x86_64.rpm`: `sudo dnf install ./apexdesk-*.rpm` |
| Arch Linux, Manjaro | `apexdesk-bin-<versión>-1-x86_64.pkg.tar.zst`: `sudo pacman -U ./apexdesk-bin-*.pkg.tar.zst` |
| Linux, portable (USB) | `apexdesk-<versión>-linux-x86_64-portable.tar.gz`: descomprime y ejecuta `./apexdesk` |

Linux necesita glibc 2.31 o posterior (Ubuntu 20.04, Debian 11, Fedora 32 y posteriores).

O instálalo directamente desde la versión publicada:

```sh
sudo pacman -U https://github.com/apexdesk/apexdesk/releases/download/v0.8.0/apexdesk-bin-0.8.0-1-x86_64.pkg.tar.zst
```

El paquete se crea desde [`aur/apexdesk-bin`](aur/apexdesk-bin); para crearlo tú con makepkg:

```sh
git clone https://github.com/apexdesk/apexdesk.git
cd apexdesk/aur/apexdesk-bin && makepkg -si
```

**Windows SmartScreen:** el programa todavía no está firmado, así que
Windows puede mostrar «Windows protegió su PC». Pulsa **Más información**
y luego **Ejecutar de todas formas**.

**Licencia:** ApexDesk es gratis, pero no es código abierto. Las
condiciones están en el [contrato de licencia](EULA.es.md): gratis sin
límite de tiempo para particulares, autónomos, empresas de menos de 10
empleados, educación, pequeños municipios, asociaciones de voluntarios y
confesiones religiosas; gratis hasta el 31 de diciembre de 2027 para las
demás empresas y organizaciones.
