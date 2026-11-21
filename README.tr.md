<p align="center"><img src="assets/app_icon.png" alt="git2pkg" width="128"></p>

[English](README.md) | [Deutsch](README.de.md) | [Türkçe](README.tr.md) | [Español](README.es.md) | [Français](README.fr.md)

# git2pkg

GitHub, GitLab ve benzeri platformlardaki Git depolarını **tek kullanımlık bir
Podman/Docker konteynerinde** derleyen ve sonucu görünür bir çıktı klasörüne
(varsayılan olarak `~/Downloads/git2pkg`) koyan küçük bir Linux masaüstü
arayüzü. Ana sistemin derleyici araç zincirine veya derleme kütüphanelerine
ihtiyacı yoktur.

`git2pkg` tamamen yerel, deterministik ve kural tabanlıdır. **İçinde yapay
zekâ, LLM veya harici API çağrısı yoktur.** Derleme kararları dosya tespiti,
derleyici çıktısı üzerinde düzenli ifadeler ve konteynerdeki paket yöneticisi
ile alınır.

> [!WARNING]
> **Alfa Durumu:** Bu proje şu anda aktif test aşamasındadır (Alfa). Klonlama ve test etme tamamen kendi sorumluluğunuzdadır. Alfa aşamasında hiçbir şekilde destek, yardım veya garanti sağlanmamaktadır — destek Beta aşamasından itibaren sunulacaktır.

## Neden bunu geliştirdim

GitHub, GitLab ve benzeri yerlerde yeni açık kaynak araçları keşfetmeyi
seviyorum; ama ana sistemimi onlarca derleme kütüphanesi ve başlık dosyasıyla
doldurmak ya da AUR gibi üçüncü taraf depolara körü körüne bağımlı olmak
istemedim. Bu yüzden `git2pkg`'i geliştirdim: Her şey geçici bir konteynerde
derleniyor, ana sistem tamamen temiz kalıyor ve sonunda yerel bir paket
(`.pkg.tar.zst`, `.deb`, `.rpm`) ya da tek bir ikili dosya çıkıyor — basit bir
arayüzle herkes kullanabiliyor.

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
  `meson.build`, `go.mod`, `pyproject.toml`, `Makefile`).
- Konteynerden canlı, kaydırılabilir derleme günlüğü ve kopyalama düğmesi.
- Eksik bağımlılık çözümü: derleyici çıktısı eksik başlıklar, pkg-config
  paketleri ve CMake modülleri için taranır; uygun paket **geçici konteynerin
  içine** kurulur ve derleme yeniden denenir (en fazla 5 deneme).
- Yerel paketler: `.pkg.tar.zst` (Arch), `.deb` (Debian/Ubuntu), `.rpm`
  (Fedora/openSUSE).
- **Çıktı klasörü**: sonuçlar varsayılan olarak `~/Downloads/git2pkg` içine
  yazılır. GUI'deki yerel klasör seçici (`rfd`) veya doğrudan yol girişiyle
  değiştirilebilir.
- Sonuç işlemleri: çıktı klasörünü aç, derleme tarifini kaydet ve bir şey
  çalıştırılmadan önce tam kurulum komutunu gör.
- İsteğe bağlı başsız mod: `git2pkg --cli <url> [--format=package|binary]`.

## Gereksinimler

- Rust 1.80+ (kaynaktan derlemek için)
- Podman (tercihen) veya Docker
- GUI için grafik oturum (Wayland veya X11)

## Kaynaktan derleme

```sh
git clone https://github.com/hyalcin70/git2pkg.git
cd git2pkg
cargo build --release
```

GUI'yi `cargo run --release`, CLI'yı
`cargo run --release -- --cli https://github.com/user/repo` ile çalıştırın.

## Masaüstü kurulumu

Proje kökünde `./install.sh` komutunu çalıştırın. Release ikilisini derler ve
**nereye** kopyalanacağını sorar: proje klasörü (varsayılan), `~/Applications`,
masaüstü veya özel bir yol. Ardından başlat menüsü girişi önerilir.
`./uninstall.sh` ikiliyi ve menü girişini kaldırır; yolu `.desktop` dosyasından
okur.

## Arch / CachyOS paketi

`packaging/PKGBUILD`, `git2pkg`'i normal bir sistem paketi olarak derler:

```sh
cd packaging
makepkg -si
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
  gui/
    app.rs           eframe/egui uygulaması
    fonts.rs         gömülü Noto yazı tipleri
    i18n.rs          EN/DE/TR/ES/FR metinleri, dil ve tema algılama
assets/              gömülü Noto Sans + Noto Sans Mono (OFL 1.1)
.github/workflows/   etiket tetikli GitHub release akışı
.gitlab-ci.yml       GitLab CI derleme ve paketleme işi
packaging/PKGBUILD   git2pkg için Arch paket tarifi
```

## CI / Sürümler

`v*` ile eşleşen bir etiket `.github/workflows/release.yml` akışını tetikler:
`ubuntu-latest` üzerinde Linux `x86_64` ikilisi derlenir,
`git2pkg-linux-x86_64.tar.gz` ve `.sha256` dosyası oluşturulur ve ikisi de
GitHub sürümüne eklenir. `.gitlab-ci.yml` aynı arşivi GitLab'da derler ve iş
çıktısı olarak sunar.

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
> **Proje Kapsamı ve Tek Kişilik Geliştirme:** `git2pkg`, boş zamanlarımda geliştirdiğim bağımsız bir tek kişilik hobi projesidir. Beta aşamasından itibaren sağlanacak destek yalnızca desteklenen ana dağıtım aileleriyle (**Arch / CachyOS**, **Debian / Ubuntu / Linux Mint**, **Fedora / Red Hat** ve **openSUSE**) sınırlıdır. Egzotik veya niş Linux dağıtımları kesinlikle desteklenmemektedir.

## Teşekkürler ve şeffaflık

Bu projeyi ben tasarladım ve yönlendirdim; geliştirirken yapay zekâ
araçlarından yardım aldım. Bunu açıkça söylemek istiyorum. Ancak bitmiş
`git2pkg` programı tamamen yerel ve deterministik çalışır; hiçbir yapay zekâ
veya harici API kullanmaz.

## Lisans

GPL-3.0-or-later — bkz. [LICENSE](LICENSE). `assets/` içindeki Noto Sans ve
Noto Sans Mono yazı tipi dosyaları SIL Open Font License 1.1 ile lisanslıdır.
