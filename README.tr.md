<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

[![CI](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml/badge.svg)](https://github.com/hyalcin70/git2pkg/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/hyalcin70/git2pkg?include_prereleases&label=release)](https://github.com/hyalcin70/git2pkg/releases)

GitHub, GitLab ve benzeri platformlardaki Git depolarından **desteklenen yerel
Linux projelerini tek kullanımlık bir Podman/Docker konteynerinde** derleyen ve
sonucu görünür bir çıktı klasörüne (varsayılan olarak `~/Downloads/git2pkg`)
koyan küçük bir Linux masaüstü arayüzü. Ana sistemin derleyici araç zincirine
veya derleme kütüphanelerine ihtiyacı yoktur.

`git2pkg` tamamen yerel, deterministik ve kural tabanlıdır. **İçinde yapay
zekâ, LLM veya harici API çağrısı yoktur.** Derleme kararları dosya tespiti,
derleyici çıktısı üzerinde düzenli ifadeler ve konteynerdeki paket yöneticisi
ile alınır.

> [!WARNING]
> **Alfa durumu:** `git2pkg` aktif geliştirme aşamasındadır. Uyumsuz değişiklikler ve tamamlanmamış özellikler beklenmelidir. Lütfen tek kullanımlık konteynerlerde test edin, yedeklerinizi saklayın ve hataları tam günlükle bildirin.

## Desteklenen kapsam

`git2pkg` yerel Linux masaüstü ve CLI projeleri için tasarlanmıştır. **Evrensel
bir derleyici değildir**: desteklenen derleme sistemleri, hedef biçimleri ve ana
dağıtımlar aşağıda listelenmiştir. Bu matrisin dışında kalanlar çalışabilir
ancak test edilen destek kapsamına girmez.

| Alan | Desteklenen |
|---|---|
| Ana sistem | Podman veya Docker bulunan x86_64 Linux |
| Hedefler | Arch/CachyOS `.pkg.tar.zst`, Debian/Ubuntu/Mint `.deb`, Fedora/RHEL ve openSUSE `.rpm`, tek ikili dosya |
| Derleme sistemleri | CMake, Meson, Autotools, Cargo, Go, Python, Make, qmake |
| Bağımlılık arama | Hedef konteynerdeki dağıtım paketleri, kurulumdan sonra doğrulanır |
| Desteklenmeyen | Gömülü/mikrodenetleyici araç zincirleri, mobil uygulamalar, web uygulamaları, IDE eklentileri, çekirdek modülleri, farklı mimari derlemeleri, konteynere kurulamayan tescilli SDK'lar, dağıtım paketleme politikalarına garantili uyum |

> **ⓘ Information**
>
> **« Açık kaynak kodu tek başına Linux desteği anlamına gelmez. »**
>
> Ancak: git2pkg, Linux'u resmi olarak hedefleyen projeleri destekler.
> Belirleyici olan projenin kendi destek beyanıdır, kaynak kodunun erişilebilir
> olması değildir.
>
> <details>
> <summary>Ayrıntı: Destek matrisinde neler var?</summary>
>
> - ✓ **Resmi Linux hedefli projeler** (git2pkg tarafından desteklenir).
> - ✗ **Yalnızca Windows/macOS için olan projeler** ve **yalnızca topluluk
>   portları** destek matrisinin dışındadır.
>
> </details>

`git2pkg`, dağıtım paketlemesinin, AUR'un, Flatpak'ın veya bir güvenlik
denetiminin yerini almaz. Güvenilir şekilde derleyebildiğini derler, gerisini
açıkça bildirir ve ortaya çıkan paket hedef dağıtımda test edilmelidir.

## Neden bunu geliştirdim

GitHub, GitLab ve benzeri yerlerde yeni açık kaynak araçları keşfetmeyi
seviyorum; ama ana sistemimi onlarca derleme kütüphanesi ve başlık dosyasıyla
doldurmak ya da AUR gibi üçüncü taraf depolara körü körüne bağımlı olmak
istemedim. Bu yüzden `git2pkg`'i geliştirdim: Derleme geçici bir konteynerde çalışıyor,
ana sistem tamamen temiz kalıyor ve sonunda yerel bir paket (`.pkg.tar.zst`,
`.deb`, `.rpm`) ya da tek bir ikili dosya çıkıyor — basit bir arayüzle
kullanılabiliyor.

## Özellikler

- Git deposu URL'sini yapıştırın, hedef biçimi seçin, **Derlemeyi başlat**a
  tıklayın.
- **Otomatik tema**: masaüstünün açık/koyu ayarını başlangıçta algılar
  (`dark-light` crate); algılanamazsa açık tema kullanılır. Sağ üstteki
  **Otomatik / Açık / Koyu** düğmesiyle elle değiştirilebilir.
- **Otomatik dil**: `LC_ALL`, `LC_MESSAGES` veya `LANG` değerine göre Almanca
  (`de*`), Türkçe (`tr*`), İspanyolca (`es*`) veya Fransızca (`fr*`), diğer
  durumlarda İngilizce. **EN / DE / TR / ES / FR** açılır listesiyle dil
  her zaman değiştirilebilir.
- **Gömülü Noto Sans / Noto Sans Mono yazı tipleri** Türkçe karakterleri
  (İ, ı, ğ, Ğ, ş, Ş) ve tüm İspanyolca/Fransızca aksanları doğru gösterir.
- `/etc/os-release` üzerinden otomatik dağıtım algılama (Arch/CachyOS,
  Ubuntu/Debian, Fedora, openSUSE) ve uygun kurulum komutları (`pacman -U`,
  `apt install`, `dnf install`, `zypper install`).
- Otomatik derleme sistemi algılama (`Cargo.toml`, `CMakeLists.txt`,
  `meson.build`, Autotools (`configure.ac`), qmake (`.pro`), `go.mod`,
  `pyproject.toml`, `Makefile`).
- Depo derleme betikleri hedef dağıtıma göre seçilir; başka dağıtımlara veya
  işletim sistemlerine ait betikler yok sayılır.
- Konteynerden canlı, kaydırılabilir derleme günlüğü ve kopyalama düğmesi.
- Başka bir stratejiyle yeniden denemek mevcut derleme çalışma alanını yeniden
  kullanır (ikinci klon yok) ve önceki başarısız günlüğü otomatik kaydeder.
- Eksik bağımlılık çözümü: derleyici çıktısı eksik başlıklar, pkg-config
  paketleri, CMake modülleri, araçlar ve bağlayıcı kitaplıkları için taranır;
  uygun paket **geçici konteynerin içine** kurulur ve gerçekten sağlayıp
  sağlamadığı doğrulanır (başlık/modül/kitaplık gerçekten var olmalı), ardından
  derleme yeniden denenir (en fazla 5 deneme). Yalnızca doğrulanmış eşleşmeler
  kalıcı olarak saklanır.
- Hata teşhisi: derlemeler sınıflandırılır (eksik bağımlılık, sürüm/ABI
  uyuşmazlığı, paketlenmiş kod, derleyici hatası, paketleme hatası). Ölümcül
  ABI veya paket içi finder çakışmaları, rastgele paketler denemek yerine
  derlemeyi net bir sürüm/ABI tanısıyla hemen durdurur.
- Yerel paketler: `.pkg.tar.zst` (Arch; doğrudan üretilir, depoda varsa
  `PKGBUILD` ile `makepkg`), `.deb` (Debian/Ubuntu), `.rpm` (Fedora/openSUSE).
  Kendi `debian/` veya `*.spec` dosyasını içeren projeler bu tarifle
  `dpkg-buildpackage`/`rpmbuild` üzerinden paketlenir.
- İsteğe bağlı hazır builder imajları (`packaging/builders/build.sh`) her
  dağıtım için tam araç zincirini sağlar ve tekrarlanan derlemeleri birkaç kat
  hızlandırır; onlar olmadan her şey eskisi gibi çalışır.
- **Çıktı klasörü**: sonuçlar varsayılan olarak `~/Downloads/git2pkg` içine
  yazılır. GUI'deki yerel klasör seçici (`rfd`) veya doğrudan yol girişiyle
  değiştirilebilir.
- Sonuç işlemleri: çıktı klasörünü aç, derleme tarifini kaydet ve bir şey
  çalıştırılmadan önce tam kurulum komutunu gör.
- İsteğe bağlı başsız mod: `git2pkg --cli <url> [--format=package|binary]`.
- Öğrenilen eşleştirmeleri `git2pkg --reset-learning` ile sıfırla (Git deposu önbelleği korunur).
- Öğrenilen bağımlılık eşleştirmelerini `git2pkg --explain-learning` ile görüntüle.

## Gereksinimler

- Rust 1.89+ (kaynaktan derlemek için)
- Podman (tercihen) veya Docker
- GUI için grafik oturum (Wayland veya X11)

## Kaynaktan derleme

```sh
# Bir kez: güncel Rust araç zinciri (dağıtımın sürümü çoğu zaman eski)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
rustup default stable
cargo build --release
```

Dağıtımın sağladığı Rust sürümü çoğu zaman gerekenden eskidir (özellikle
Debian/Ubuntu/Mint). `rustup default stable` güncel bir araç zinciri kullanmanızı
sağlar.

GUI'yi `cargo run --release`, CLI'yı
`cargo run --release -- --cli https://github.com/user/repo` ile çalıştırın.

## Masaüstü kurulumu

Proje kökünde `./install.sh` komutunu çalıştırın. Release ikilisini derler ve
**nereye** kopyalanacağını sorar: proje klasörü (varsayılan), `~/Applications`,
masaüstü veya özel bir yol. Ardından başlat menüsü girişi önerilir.
`./uninstall.sh` ikiliyi ve menü girişini kaldırır; yolu `.desktop` dosyasından
okur.

## Yerel paketler

`git2pkg` uygulamasını dağıtımınız için normal bir sistem paketi olarak derleyin.
Tüm tarifler mevcut checkout'u derler ve Rust 1.89 veya üstünü gerektirir.

### Arch / CachyOS

```sh
cd packaging
makepkg -sc      # paketi derler ve src/ ile pkg/ dizinlerini siler
# makepkg -sic   # aynısı, ardından kurulum
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

Dağıtımın Rust sürümü 1.89'dan eskiyse önce `rustup` ile güncel kararlı
araç zincirini kurun.

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

## Proje yapısı

```
src/
  main.rs            giriş noktası, GUI / CLI ayrımı
  cli.rs             başsız mod
  core/              saf mantık, UI kodu yok
    distro.rs        /etc/os-release ayrıştırma
    detector.rs      derleme sistemi algılama
    resolver.rs      regex hata analizi + arama komutları
    container.rs     konteyner yönetimi ve paketleme
    packaging/       Arch/Debian/RPM paketleme arka uçları
  gui/
    app.rs           eframe/egui uygulaması
    fonts.rs         gömülü Noto yazı tipleri
    i18n.rs          EN/DE/TR/ES/FR metinleri, dil ve tema algılama
    settings.rs      GUI ayarları (çıktı klasörü, tema)
assets/              yazı tipleri, simge, bağımlılık bilgi tabanı
scripts/             elle duman testi korpusu (argüman olarak depo URL'leri)
packaging/           Arch PKGBUILD, RPM spec/build betiği ve masaüstü dosyası
debian/              Debian/Ubuntu paketleme (dpkg-buildpackage)
.github/workflows/   CI ve etiket tetikli release akışları
.gitlab-ci.yml       GitLab CI derleme ve paketleme işi
```

## CI / Sürümler

`v*` ile eşleşen bir etiket `.github/workflows/release.yml` akışını tetikler:
`ubuntu-latest` üzerinde Linux `x86_64` ikilisi derlenir,
`git2pkg-linux-x86_64.tar.gz` ve `.sha256` dosyası oluşturulur ve ikisi de
GitHub sürümüne eklenir. `.gitlab-ci.yml` aynı arşivi GitLab'da derler ve iş
çıktısı olarak sunar. Alfa, beta ve sürüm adayı etiketleri GitHub ön sürümü
olarak yayımlanır.

## Katkı ve çeviriler

Katkılar memnuniyetle karşılanır. Pull request akışı, `cargo fmt` / `cargo
clippy` kuralları ve yeni derleme sistemleri için
[CONTRIBUTING.md](CONTRIBUTING.md) dosyasına bakın.

### Yeni dil eklemek

Tüm kullanıcı metinleri `src/gui/i18n.rs` dosyasındadır:

1. `Language` değişkeni ekleyin ve `Language::ALL` listesine alın — bu dizi
   araç çubuğundaki listeyi belirler.
2. `Language::detect()` içine yerel ayar önekini (`de`, `tr`, `es`, `fr`, ...)
   ve kısa kodu/etiketi ekleyin.
3. Yeni çeviriyi **her** `Texts::get(...)` çağrısına ekleyin. Sıra her zaman
   `EN, DE, TR, ES, FR, <yeni dil>` şeklindedir.
4. `assets/` içindeki yazı tiplerinin yeni dilin karakterlerini içerdiğini
   doğrulayın.
5. PR açmadan önce `cargo fmt`, `cargo clippy` ve `cargo test` çalıştırın.

### Başarısız derleme bildirmek

GUI'de **Günlüğü kopyala**ya tıklayın ve **Build failure** şablonuyla yeni bir
issue açın. **Tam** günlüğü, depo URL'sini, dağıtımı ve Podman/Docker
kullandığınızı belirtin. Yalnızca son satırın ekran görüntüsü genellikle
yeterli değildir.

## Destek

Destek tamamen gönüllüdür. `git2pkg` size zaman kazandırdıysa depoya yıldız
verin, paylaşın, kopyalanan günlükle bir hata bildirin veya bir çeviri
katkısında bulunun. Telemetri yok, zorunluluk yok — araç bunların hiçbiri
olmadan da aynı şekilde çalışır.

> [!IMPORTANT]
> **Proje Kapsamı ve Tek Kişilik Geliştirme:** `git2pkg`, boş zamanlarımda geliştirdiğim bağımsız bir tek kişilik hobi projesidir. Beta aşamasından itibaren sağlanacak destek yalnızca desteklenen ana dağıtım aileleriyle (**Arch / CachyOS**, **Debian / Ubuntu / Linux Mint**, **Fedora / Red Hat** ve **openSUSE**) sınırlıdır. Diğer dağıtımlar çalışabilir ancak test edilen destek matrisinin dışındadır.

## Teşekkürler ve şeffaflık

Bu projenin fikri bana ait; yönünü belirledim, testlerini yaptım ve hataları analiz ettim — ancak kaynak kodun neredeyse tamamı yapay zeka araçlarının yardımıyla oluşturuldu. Bunu özellikle açık ve dürüstçe belirtmek istiyorum. Bununla birlikte, ortaya çıkan `git2pkg` programının kendisi hiçbir yapay zeka veya harici API kullanmadan %100 yerel ve deterministik olarak çalışır.

## Lisans

GPL-3.0-or-later — bkz. [LICENSE](LICENSE). `assets/` içindeki Noto Sans ve
Noto Sans Mono yazı tipi dosyaları SIL Open Font License 1.1 ile lisanslıdır.

## Teşekkürler

<a href="https://github.com/NousResearch/hermes-agent"><img src="assets/badges/hermes-agent.svg" alt="Hermes Agent" height="28"></a>

`git2pkg` geliştirilirken yerel bir geliştirme aracı olarak yardımcı olan Nous Research ekibinin [Hermes Agent](https://github.com/NousResearch/hermes-agent) projesine özel teşekkürler (`git2pkg` uygulamasının kendisi ise herhangi bir yapay zeka bağımlılığı olmadan %100 deterministik olarak çalışır).
