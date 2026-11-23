<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

[![CI](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml/badge.svg)](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/hyalcin70/git2pkg?include_prereleases&label=release)](https://github.com/hyalcin70/git2pkg/releases)

Una pequeña interfaz de escritorio Linux que compila **proyectos nativos Linux
compatibles** desde repositorios Git **dentro de un contenedor desechable
(Podman/Docker)** y coloca el resultado en una carpeta de salida visible
(`~/Downloads/git2pkg` por defecto). El sistema anfitrión nunca necesita una
cadena de compilación ni bibliotecas de compilación.

`git2pkg` funciona al 100 % en local, de forma determinista y basada en reglas.
**No contiene IA, LLM ni llamadas a APIs externas.** Las decisiones de compilación
se toman mediante detección de archivos, expresiones regulares sobre la salida
del compilador y el gestor de paquetes del contenedor.

> [!WARNING]
> **Estado alfa:** `git2pkg` está en desarrollo activo. Es previsible que haya cambios incompatibles y funciones incompletas. Pruebe en contenedores desechables, conserve copias de seguridad e informe de los fallos con el registro completo.

## Alcance compatible

`git2pkg` está pensado para proyectos nativos de escritorio y CLI en Linux. **No
es un constructor universal**: los sistemas de compilación, formatos de destino
y distribuciones anfitrionas compatibles se enumeran abajo. Lo que quede fuera
de esta matriz puede funcionar, pero no forma parte del alcance de soporte
probado.

| Área | Compatible |
|---|---|
| Anfitrión | Linux x86_64 con Podman o Docker |
| Destinos | Arch/CachyOS `.pkg.tar.zst`, Debian/Ubuntu/Mint `.deb`, Fedora/RHEL y openSUSE `.rpm`, binario único |
| Sistemas de compilación | CMake, Meson, Autotools, Cargo, Go, Python, Make, qmake |
| Búsqueda de dependencias | Paquetes de la distribución dentro del contenedor de destino, verificados tras la instalación |
| No compatible | Cadenas de herramientas embebidas/microcontroladores, apps móviles, apps web, complementos de IDE, módulos del kernel, compilaciones para otra arquitectura, SDK propietarios que no se puedan instalar en el contenedor, cumplimiento garantizado de las políticas de empaquetado de la distribución |

`git2pkg` no sustituye al empaquetado de la distribución, AUR, Flatpak ni una
auditoría de seguridad. Compila lo que puede compilar de forma fiable, informa
claramente del resto y el paquete resultante debe probarse en la distribución
de destino.

## Por qué lo construí

Me encanta descubrir nuevas herramientas de código abierto en GitHub, GitLab y
otros sitios, pero no quería llenar mi sistema principal con decenas de
bibliotecas de compilación y cabeceras de desarrollo, ni depender a ciegas de
repositorios de terceros como AUR. Por eso construí `git2pkg`: la compilación se ejecuta en un contenedor temporal
desechable, el sistema anfitrión permanece completamente limpio y al final se
obtiene un paquete nativo (`.pkg.tar.zst`, `.deb`, `.rpm`) o un único binario,
utilizable desde una interfaz gráfica sencilla.

## Características

- Pegue la URL de un repositorio Git, elija el formato de destino y pulse
  **Iniciar compilación**.
- **Tema automático**: detecta al inicio la preferencia clara/oscura del
  escritorio (mediante `dark-light`); si no se puede determinar, usa el tema
  claro. Un botón **Automático / Claro / Oscuro** arriba a la derecha lo anula.
- **Idioma automático**: alemán (`de*`), turco (`tr*`), español (`es*`) o
  francés (`fr*`) según `LC_ALL`, `LC_MESSAGES` o `LANG`; inglés en cualquier
  otro caso. Un desplegable **EN / DE / TR / ES / FR** permite cambiarlo.
- **Fuentes Noto Sans / Noto Sans Mono integradas**: muestran correctamente los
  caracteres turcos (İ, ı, ğ, Ğ, ş, Ş) y todos los acentos españoles y franceses.
- Detección automática del sistema desde `/etc/os-release` (Arch/CachyOS,
  Ubuntu/Debian, Fedora, openSUSE) con comandos de instalación adecuados
  (`pacman -U`, `apt install`, `dnf install`, `zypper install`).
- Detección automática del sistema de compilación (`Cargo.toml`, `CMakeLists.txt`,
  `meson.build`, Autotools (`configure.ac`), qmake (`.pro`), `go.mod`,
  `pyproject.toml`, `Makefile`).
- Registro de compilación en vivo y desplazable, con botón de copia.
- Resolución de dependencias: la salida del compilador se analiza en busca de
  cabeceras, paquetes pkg-config, módulos CMake, herramientas y bibliotecas de
  enlace faltantes; el paquete
  correspondiente se instala **dentro del contenedor desechable** y se verifica
  (la cabecera, el módulo o la biblioteca debe existir realmente) antes de
  reintentar la compilación (máx. 5 intentos). Solo se memorizan las
  coincidencias verificadas.
- Diagnóstico de fallos: las compilaciones se clasifican (dependencia ausente,
  desajuste de versión/ABI, código empaquetado, error del compilador, error de
  empaquetado). Si los paquetes no pueden ayudar, la compilación se detiene con
  un diagnóstico claro en lugar de probar paquetes al azar.
- Paquetes nativos: `.pkg.tar.zst` (Arch; generado directamente, `PKGBUILD` del
  repositorio mediante `makepkg` si existe), `.deb` (Debian/Ubuntu), `.rpm`
  (Fedora/openSUSE).
- **Carpeta de salida**: los resultados van a `~/Downloads/git2pkg` por defecto.
  Cámbiela en la GUI con el selector nativo (`rfd`) o escribiendo una ruta.
- Acciones de resultado: abrir la carpeta de salida, guardar la receta de
  compilación y ver el comando exacto de instalación antes de ejecutarlo.
- Modo headless opcional: `git2pkg --cli <url> [--format=package|binary]`.
- Restablece las asignaciones aprendidas con `git2pkg --reset-learning` (la caché del repositorio Git se conserva).
- Consulta las asignaciones de dependencias aprendidas con `git2pkg --explain-learning`.

## Requisitos

- Rust 1.89+ (para compilar desde el código fuente)
- Podman (preferido) o Docker
- Una sesión gráfica (Wayland o X11) para la GUI

## Compilación desde el código fuente

```sh
git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
cargo build --release
```

Ejecute la GUI con `cargo run --release`, o la CLI con
`cargo run --release -- --cli https://github.com/user/repo`.

## Instalación de escritorio

Desde la raíz del repositorio, ejecute `./install.sh`. El script compila el
binario de release y pregunta **dónde** colocarlo: la carpeta del proyecto
(predeterminado), `~/Applications`, el escritorio o una ruta personalizada.
Después ofrece crear una entrada en el menú de aplicaciones. `./uninstall.sh`
elimina el binario y la entrada leyendo la ruta del archivo `.desktop`.

## Paquetes nativos

Compile `git2pkg` como paquete del sistema para su distribución. Todas las
recetas compilan el checkout actual y requieren Rust 1.89 o superior.

### Arch / CachyOS

```sh
cd packaging
makepkg -sc      # compila el paquete y elimina src/ y pkg/
# makepkg -sic   # igual, pero instalándolo
```

### Debian / Ubuntu

```sh
sudo apt install debhelper dpkg-dev build-essential cargo rustc pkg-config \
  libxkbcommon-dev libwayland-dev libxcb1-dev libxrandr-dev libxi-dev \
  libxcursor-dev libx11-dev libgl1-mesa-dev libatk1.0-dev libgtk-3-dev \
  libasound2-dev
dpkg-buildpackage -us -uc -b
sudo apt install ../git2pkg_*.deb
```

Si el Rust de la distribución es anterior a 1.89, instale primero la cadena
estable actual con `rustup`.

### Fedora / RHEL

```sh
sudo dnf install rpm-build rpmdevtools cargo rust pkgconf-pkg-config dnf-plugins-core
sudo dnf builddep packaging/git2pkg.spec
./packaging/build-rpm.sh
sudo dnf install ~/rpmbuild/RPMS/*/git2pkg-*.rpm
```

### openSUSE

```sh
sudo zypper install rpm-build cargo rust pkgconf-pkg-config \
  libxkbcommon-devel wayland-devel libxcb-devel libXrandr-devel \
  libXi-devel libXcursor-devel Mesa-libGL-devel atk-devel gtk3-devel alsa-devel
./packaging/build-rpm.sh
sudo zypper install ~/rpmbuild/RPMS/*/git2pkg-*.rpm
```

## Estructura del proyecto

```
src/
  main.rs            punto de entrada, GUI frente a CLI
  cli.rs             modo headless
  core/              lógica pura, sin código de UI
    distro.rs        análisis de /etc/os-release
    detector.rs      detección del sistema de compilación
    resolver.rs      análisis regex de errores + comandos de búsqueda
    container.rs     orquestación de contenedores y empaquetado
    packaging/       backends de empaquetado Arch/Debian/RPM
  gui/
    app.rs           aplicación eframe/egui
    fonts.rs         fuentes Noto integradas
    i18n.rs          textos EN/DE/TR/ES/FR y detección de idioma/tema
    settings.rs      ajustes de la GUI (carpeta de salida, tema)
assets/              fuentes, icono, base de conocimiento de dependencias
scripts/             corpus de pruebas manual (URLs como argumentos)
packaging/           PKGBUILD de Arch, spec/script RPM y archivo .desktop
debian/              empaquetado Debian/Ubuntu (dpkg-buildpackage)
.github/workflows/   flujos de CI y release por etiquetas
.gitlab-ci.yml       compilación y empaquetado en GitLab CI
```

## CI / Releases

Al subir una etiqueta `v*` se ejecuta `.github/workflows/release.yml`: compila
el binario Linux `x86_64` en `ubuntu-latest`, genera
`git2pkg-linux-x86_64.tar.gz` y su `.sha256`, y adjunta ambos al release de
GitHub. `.gitlab-ci.yml` genera el mismo tarball en GitLab y lo publica como
artefacto del trabajo. Las etiquetas alfa, beta y release candidate se publican
como pre-releases de GitHub.

## Contribuciones y traducciones

Se agradecen las contribuciones. Consulte [CONTRIBUTING.md](CONTRIBUTING.md)
para el flujo de pull requests, `cargo fmt` / `cargo clippy` y cómo añadir
nuevos sistemas de compilación.

### Añadir un idioma

Todas las cadenas visibles están en `src/gui/i18n.rs`:

1. Añada una variante a `Language` e inclúyala en `Language::ALL`; esa lista
   controla el desplegable de la barra de herramientas.
2. Amplíe `Language::detect()` con el prefijo de locale (`de`, `tr`, `es`, `fr`,
   ...) y su código y etiqueta.
3. Añada la traducción a **todas** las llamadas `Texts::get(...)`. El orden es
   siempre `EN, DE, TR, ES, FR, <nuevo idioma>`.
4. Verifique que las fuentes de `assets/` contengan los caracteres del nuevo
   idioma.
5. Ejecute `cargo fmt`, `cargo clippy` y `cargo test` antes del PR.

### Informar de un error de compilación

Pulse **Copiar registro** en la GUI y abra un issue con la plantilla
**Build failure**. Pegue el registro **completo**, la URL del repositorio, su
distribución y si usa Podman o Docker. Una captura de solo la última línea
suele ser insuficiente.

## Apoyo

El apoyo es totalmente voluntario. Si `git2pkg` le ahorra tiempo, dé una estrella
al repositorio, compártalo, informe de un error con el registro copiado o
contribuya con una traducción. No hay telemetría ni obligación: la herramienta
funciona exactamente igual con o sin todo ello.

> [!IMPORTANT]
> **Alcance del proyecto y desarrollo individual:** `git2pkg` es un proyecto independiente desarrollado en mi tiempo libre por una sola persona. A partir de la fase Beta, el soporte (según disponibilidad) se centrará estrictamente en las principales familias de distribuciones compatibles (**Arch / CachyOS**, **Debian / Ubuntu / Linux Mint**, **Fedora / Red Hat** y **openSUSE**). Otras distribuciones pueden funcionar, pero quedan fuera de la matriz de soporte probada.

## Créditos y transparencia

Tuve la idea de este proyecto, dirigí su rumbo, realicé las pruebas y analicé los errores, mientras que casi todo el código fuente fue escrito con la ayuda de herramientas de IA. Quiero decirlo de forma abierta y honesta. Sin embargo, el programa `git2pkg` terminado funciona de manera 100 % local y determinista, sin ningún tipo de IA ni API externas.

## Licencia

GPL-3.0-or-later: consulte [LICENSE](LICENSE). Las fuentes Noto Sans y Noto Sans
Mono incluidas en `assets/` están bajo la SIL Open Font License 1.1.

## Agradecimientos

<a href="https://github.com/NousResearch/hermes-agent"><img src="assets/badges/hermes-agent.svg" alt="Hermes Agent" height="28"></a>

Un agradecimiento especial a [Hermes Agent](https://github.com/NousResearch/hermes-agent) de Nous Research, que ayudó como herramienta de desarrollo local durante la creación de `git2pkg` (mientras que `git2pkg` en sí funciona de forma 100 % determinista y sin dependencias de IA).
