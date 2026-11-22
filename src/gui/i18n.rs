//! Zentrale Internationalisierung (EN/DE/TR/ES/FR) und Theme-Verwaltung.
//!
//! Alle Texte – GUI wie Build-Orchestrierung – liegen in dieser Datei.
//! Die Systemsprache wird beim Start aus `LC_ALL`, `LC_MESSAGES` und `LANG`
//! ermittelt; `de*` → Deutsch, `tr*` → Türkisch, `es*` → Spanisch,
//! `fr*` → Französisch, alles andere → Englisch.

use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    De,
    Tr,
    Es,
    Fr,
}

impl Language {
    /// Erkennt die Systemsprache aus den üblichen Locale-Variablen.
    pub fn detect() -> Self {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = env::var(key) {
                let value = value.trim().to_lowercase();
                if value.is_empty() || value == "c" || value == "posix" {
                    continue;
                }
                if value.starts_with("de") {
                    return Language::De;
                }
                if value.starts_with("tr") {
                    return Language::Tr;
                }
                if value.starts_with("es") {
                    return Language::Es;
                }
                if value.starts_with("fr") {
                    return Language::Fr;
                }
            }
        }
        Language::En
    }

    /// Alle unterstützten Sprachen in der Reihenfolge des Dropdowns.
    pub const ALL: [Language; 5] = [
        Language::En,
        Language::De,
        Language::Tr,
        Language::Es,
        Language::Fr,
    ];

    #[allow(dead_code)]
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::De => "de",
            Language::Tr => "tr",
            Language::Es => "es",
            Language::Fr => "fr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::En => "EN",
            Language::De => "DE",
            Language::Tr => "TR",
            Language::Es => "ES",
            Language::Fr => "FR",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Auto,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn next(self) -> Self {
        match self {
            ThemeMode::Auto => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Auto,
        }
    }

    /// `Auto` fragt das System; ein nicht ermittelbares Theme gilt als hell.
    pub fn is_dark(self) -> bool {
        match self {
            ThemeMode::Auto => dark_light::detect() == dark_light::Mode::Dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        }
    }
}

pub struct Texts {
    pub lang: Language,
}

impl Texts {
    pub fn new(lang: Language) -> Self {
        Self { lang }
    }

    /// Reihenfolge: EN, DE, TR, ES, FR.
    pub fn get(
        &self,
        en: &'static str,
        de: &'static str,
        tr: &'static str,
        es: &'static str,
        fr: &'static str,
    ) -> &'static str {
        match self.lang {
            Language::En => en,
            Language::De => de,
            Language::Tr => tr,
            Language::Es => es,
            Language::Fr => fr,
        }
    }

    // ---------- GUI ----------

    pub fn subtitle(&self) -> &'static str {
        self.get(
            "Isolated package builds from Git repositories",
            "Isolierte Paket-Builds aus Git-Repositories",
            "Git depolarından izole paket derlemeleri",
            "Compilación aislada de paquetes desde repositorios Git",
            "Compilation isolée de paquets depuis des dépôts Git",
        )
    }
    pub fn repo_url(&self) -> &'static str {
        self.get(
            "Repository URL",
            "Repository-URL",
            "Depo bağlantısı",
            "URL del repositorio",
            "URL du dépôt",
        )
    }
    pub fn clear_url(&self) -> &'static str {
        self.get(
            "Clear URL",
            "URL löschen",
            "URL'yi temizle",
            "Borrar URL",
            "Effacer l'URL",
        )
    }
    pub fn repo_hint(&self) -> &'static str {
        self.get(
            "Git URL of the repository",
            "Git-URL des Repositories",
            "Depo Git URL'si",
            "URL Git del repositorio",
            "URL Git du dépôt",
        )
    }
    pub fn build_start(&self) -> &'static str {
        self.get(
            "Start build",
            "Build starten",
            "Derlemeyi başlat",
            "Iniciar compilación",
            "Lancer la compilation",
        )
    }
    pub fn engine_missing_title(&self) -> &'static str {
        self.get(
            "No container engine found",
            "Keine Container-Engine gefunden",
            "Konteyner motoru bulunamadı",
            "No se encontró ningún motor de contenedores",
            "Aucun moteur de conteneurs trouvé",
        )
    }
    pub fn engine_missing_text(&self) -> &'static str {
        self.get(
            "Podman or Docker is required to run builds. Please install one of them and restart git2pkg.",
            "Für Builds wird Podman oder Docker benötigt. Bitte installiere eine der beiden Engines und starte git2pkg neu.",
            "Derleme yapmak için Podman veya Docker gerekir. Lütfen ikisinden birini kurun ve git2pkg'i yeniden başlatın.",
            "Se necesita Podman o Docker para compilar. Instala uno de ellos y vuelve a iniciar git2pkg.",
            "Podman ou Docker est nécessaire pour compiler. Installez l'un des deux puis redémarrez git2pkg.",
        )
    }
    pub fn copy(&self) -> &'static str {
        self.get("Copy", "Kopieren", "Kopyala", "Copiar", "Copier")
    }
    pub fn target_format(&self) -> &'static str {
        self.get(
            "Target format:",
            "Zielformat:",
            "Hedef biçim:",
            "Formato de destino:",
            "Format cible :",
        )
    }

    /// Anzeige des automatisch gewählten nativen Paketziels.
    #[allow(dead_code)]
    pub fn native_package_badge(&self, extension: &str, distro: &str) -> String {
        match self.lang {
            Language::En => format!("Native system package ({extension} \u{2014} {distro})"),
            Language::De => format!("Natives Systempaket ({extension} \u{2014} {distro})"),
            Language::Tr => format!("Yerel sistem paketi ({extension} \u{2014} {distro})"),
            Language::Es => format!("Paquete del sistema nativo ({extension} \u{2014} {distro})"),
            Language::Fr => format!("Paquet système natif ({extension} \u{2014} {distro})"),
        }
    }

    pub fn target_auto(&self, extension: &str, distro: &str) -> String {
        match self.lang {
            Language::En => {
                format!("Auto \u{2014} native system package ({extension} \u{2014} {distro})")
            }
            Language::De => {
                format!("Auto \u{2014} Natives Systempaket ({extension} \u{2014} {distro})")
            }
            Language::Tr => {
                format!("Otomatik \u{2014} yerel sistem paketi ({extension} \u{2014} {distro})")
            }
            Language::Es => {
                format!("Auto \u{2014} paquete del sistema nativo ({extension} \u{2014} {distro})")
            }
            Language::Fr => {
                format!("Auto \u{2014} paquet système natif ({extension} \u{2014} {distro})")
            }
        }
    }

    pub fn target_arch(&self) -> &'static str {
        self.get(
            "Arch Linux / CachyOS (.pkg.tar.zst)",
            "Arch Linux / CachyOS (.pkg.tar.zst)",
            "Arch Linux / CachyOS (.pkg.tar.zst)",
            "Arch Linux / CachyOS (.pkg.tar.zst)",
            "Arch Linux / CachyOS (.pkg.tar.zst)",
        )
    }

    pub fn target_debian(&self) -> &'static str {
        self.get(
            "Debian / Ubuntu / Linux Mint (.deb)",
            "Debian / Ubuntu / Linux Mint (.deb)",
            "Debian / Ubuntu / Linux Mint (.deb)",
            "Debian / Ubuntu / Linux Mint (.deb)",
            "Debian / Ubuntu / Linux Mint (.deb)",
        )
    }

    pub fn target_fedora(&self) -> &'static str {
        self.get(
            "Fedora / RHEL (.rpm)",
            "Fedora / RHEL (.rpm)",
            "Fedora / RHEL (.rpm)",
            "Fedora / RHEL (.rpm)",
            "Fedora / RHEL (.rpm)",
        )
    }

    pub fn target_suse(&self) -> &'static str {
        self.get(
            "openSUSE (.rpm)",
            "openSUSE (.rpm)",
            "openSUSE (.rpm)",
            "openSUSE (.rpm)",
            "openSUSE (.rpm)",
        )
    }

    pub fn cross_build_hint(&self, distro: &str) -> String {
        match self.lang {
            Language::En => format!("The package for {distro} is ready in the output folder. It cannot be installed on this system."),
            Language::De => format!("Das Paket für {distro} liegt im Ausgabeordner bereit. Es kann auf diesem System nicht installiert werden."),
            Language::Tr => format!("{distro} için paket çıktı klasöründe hazır. Bu sistemde kurulamaz."),
            Language::Es => format!("El paquete para {distro} está listo en la carpeta de salida. No se puede instalar en este sistema."),
            Language::Fr => format!("Le paquet pour {distro} est prêt dans le dossier de sortie. Il ne peut pas être installé sur ce système."),
        }
    }

    pub fn open_output_folder(&self) -> &'static str {
        self.get(
            "Open output folder",
            "Ausgabeordner öffnen",
            "Çıktı klasörünü aç",
            "Abrir carpeta de salida",
            "Ouvrir le dossier de sortie",
        )
    }

    pub fn close_only(&self) -> &'static str {
        self.get("Close", "Schließen", "Kapat", "Cerrar", "Fermer")
    }

    pub fn strategy_default(&self) -> &'static str {
        self.get("Standard", "Standard", "Standart", "Estándar", "Standard")
    }

    pub fn strategy_system_libs(&self) -> &'static str {
        self.get(
            "System libraries without deps/ prebuild",
            "System-Bibliotheken ohne deps/-Vorbau",
            "Sistem kitaplıkları, deps/ ön derlemesi yok",
            "Bibliotecas del sistema sin preconstrucción de deps/",
            "Bibliothèques système sans pré-compilation de deps/",
        )
    }

    pub fn strategy_repo_script(&self) -> &'static str {
        self.get(
            "Repository build script",
            "Repository-Buildskript",
            "Depo derleme betiği",
            "Script de compilación del repositorio",
            "Script de compilation du dépôt",
        )
    }

    pub fn strategy_direct(&self) -> &'static str {
        self.get(
            "Direct build-system fallback",
            "Direkter Build-System-Fallback",
            "Doğrudan derleme sistemi yedeği",
            "Respaldo directo del sistema de compilación",
            "Repli direct du système de compilation",
        )
    }

    pub fn next_strategy(&self, label: &str) -> String {
        match self.lang {
            Language::En => format!("Next strategy: {label}"),
            Language::De => format!("Nächste Strategie: {label}"),
            Language::Tr => format!("Sonraki strateji: {label}"),
            Language::Es => format!("Siguiente estrategia: {label}"),
            Language::Fr => format!("Prochaine stratégie : {label}"),
        }
    }
    pub fn output_folder(&self) -> &'static str {
        self.get(
            "Output folder:",
            "Ausgabeordner:",
            "Çıktı klasörü:",
            "Carpeta de salida:",
            "Dossier de sortie :",
        )
    }
    pub fn browse(&self) -> &'static str {
        self.get(
            "Browse...",
            "Ändern...",
            "Değiştir...",
            "Examinar...",
            "Parcourir...",
        )
    }
    pub fn output_dir_error(&self) -> &'static str {
        self.get(
            "Could not create output folder: {}",
            "Ausgabeordner konnte nicht erstellt werden: {}",
            "Çıktı klasörü oluşturulamadı: {}",
            "No se pudo crear la carpeta de salida: {}",
            "Impossible de créer le dossier de sortie : {}",
        )
    }
    pub fn error_empty_url(&self) -> &'static str {
        self.get(
            "Please enter a GitHub URL.",
            "Bitte eine GitHub-URL eingeben.",
            "Lütfen bir GitHub URL'si girin.",
            "Introduzca una URL de GitHub.",
            "Veuillez saisir une URL GitHub.",
        )
    }
    pub fn error_bad_scheme(&self) -> &'static str {
        self.get(
            "The URL must start with https://.",
            "Die URL muss mit https:// beginnen.",
            "URL, https:// ile başlamalıdır.",
            "La URL debe empezar por https://.",
            "L'URL doit commencer par https://.",
        )
    }
    pub fn live_log(&self) -> &'static str {
        self.get(
            "Live log",
            "Live-Log",
            "Canlı günlük",
            "Registro en vivo",
            "Journal en direct",
        )
    }

    pub fn cancel_build(&self) -> &'static str {
        self.get(
            "Cancel build",
            "Build abbrechen",
            "Derlemeyi iptal et",
            "Cancelar compilación",
            "Annuler la compilation",
        )
    }

    pub fn ok_understood(&self) -> &'static str {
        self.get(
            "OK, got it",
            "OK, verstanden",
            "Tamam, anladım",
            "Entendido",
            "OK, compris",
        )
    }

    pub fn cross_build_warning(&self) -> &'static str {
        self.get(
            "Note: Cross-builds for another distribution may work, but cannot be guaranteed for all projects. If something fails, please test building directly on the target distribution.",
            "Hinweis: Cross-Builds für eine andere Distribution können funktionieren, lassen sich aber nicht für alle Projekte garantieren. Falls es hakt, teste den Bau bitte direkt auf der jeweiligen Ziel-Distribution.",
            "Not: Başka bir dağıtım için çapraz derleme çalışabilir ancak tüm projeler için garanti edilemez. Bir sorun olursa lütfen hedef dağıtımda doğrudan derlemeyi deneyin.",
            "Nota: Las compilaciones cruzadas para otra distribución pueden funcionar, pero no se pueden garantizar para todos los proyectos. Si algo falla, prueba a compilar directamente en la distribución de destino.",
            "Remarque : les compilations croisées pour une autre distribution peuvent fonctionner, mais ne sont pas garanties pour tous les projets. En cas de problème, veuillez tester la compilation directement sur la distribution cible.",
        )
    }

    pub fn build_running(&self) -> &'static str {
        self.get(
            "Build running in container \u{2026}",
            "Build läuft im Container \u{2026}",
            "Derleme konteynerda çalışıyor \u{2026}",
            "Compilación en ejecución en el contenedor \u{2026}",
            "Compilation en cours dans le conteneur \u{2026}",
        )
    }
    pub fn copy_log(&self) -> &'static str {
        self.get(
            "Copy log",
            "Log kopieren",
            "Günlüğü kopyala",
            "Copiar registro",
            "Copier le journal",
        )
    }
    pub fn save_log(&self) -> &'static str {
        self.get(
            "Save log",
            "Log speichern",
            "Günlüğü kaydet",
            "Guardar registro",
            "Enregistrer le journal",
        )
    }
    pub fn log_saved(&self) -> &'static str {
        self.get(
            "Log saved: {}",
            "Log gespeichert: {}",
            "Günlük kaydedildi: {}",
            "Registro guardado: {}",
            "Journal enregistré : {}",
        )
    }
    pub fn log_save_failed(&self) -> &'static str {
        self.get(
            "Could not save log: {}",
            "Log konnte nicht gespeichert werden: {}",
            "Günlük kaydedilemedi: {}",
            "No se pudo guardar el registro: {}",
            "Impossible d’enregistrer le journal : {}",
        )
    }
    pub fn context_copy(&self) -> &'static str {
        self.get("Copy", "Kopieren", "Kopyala", "Copiar", "Copier")
    }
    pub fn context_select_all(&self) -> &'static str {
        self.get(
            "Select all",
            "Alles auswählen",
            "Tümünü seç",
            "Seleccionar todo",
            "Tout sélectionner",
        )
    }
    pub fn context_paste(&self) -> &'static str {
        self.get("Paste", "Einfügen", "Yapıştır", "Pegar", "Coller")
    }
    pub fn context_cut(&self) -> &'static str {
        self.get("Cut", "Ausschneiden", "Kes", "Cortar", "Couper")
    }
    #[allow(dead_code)]
    pub fn retry_alternative(&self) -> &'static str {
        self.get(
            "Retry (Different Strategy)",
            "Erneut versuchen (Andere Strategie)",
            "Yeniden dene (Farklı strateji)",
            "Reintentar (Otra estrategia)",
            "Réessayer (Autre stratégie)",
        )
    }
    pub fn ready(&self) -> &'static str {
        self.get("Ready", "Bereit", "Hazır", "Listo", "Prêt")
    }
    pub fn starting(&self) -> &'static str {
        self.get(
            "Starting …",
            "Startet …",
            "Başlatılıyor …",
            "Iniciando …",
            "Démarrage …",
        )
    }
    pub fn success_step(&self) -> &'static str {
        self.get(
            "Completed successfully",
            "Erfolgreich abgeschlossen",
            "Başarıyla tamamlandı",
            "Completado correctamente",
            "Terminé avec succès",
        )
    }
    pub fn failed_step(&self) -> &'static str {
        self.get("Failed", "Fehlgeschlagen", "Başarısız", "Fallido", "Échec")
    }
    pub fn thread_error(&self) -> &'static str {
        self.get(
            "The build thread ended unexpectedly.",
            "Build-Thread wurde unerwartet beendet.",
            "Derleme iş parçacığı beklenmedik şekilde sonlandı.",
            "El hilo de compilación terminó inesperadamente.",
            "Le thread de compilation s'est terminé de manière inattendue.",
        )
    }
    pub fn result(&self) -> &'static str {
        self.get("Result", "Ergebnis", "Sonuç", "Resultado", "Résultat")
    }
    pub fn open_dir(&self) -> &'static str {
        self.get(
            "Open output folder (./dist/)",
            "Zielordner öffnen (./dist/)",
            "Çıktı klasörünü aç (./dist/)",
            "Abrir carpeta de salida (./dist/)",
            "Ouvrir le dossier de sortie (./dist/)",
        )
    }
    pub fn save_recipe(&self) -> &'static str {
        self.get(
            "Save build recipe",
            "Build-Rezept speichern",
            "Derleme tarifini kaydet",
            "Guardar receta de compilación",
            "Enregistrer la recette de compilation",
        )
    }
    pub fn install_package(&self) -> &'static str {
        self.get(
            "Install package",
            "Paket installieren",
            "Paketi kur",
            "Instalar paquete",
            "Installer le paquet",
        )
    }
    pub fn recipe_hint(&self) -> &'static str {
        self.get(
            "Recipe copied to clipboard (build script stays in the cache).",
            "Rezept in Zwischenablage kopiert (Build-Skript bleibt im Cache).",
            "Tarif panoya kopyalandı (derleme betiği önbellekte kalır).",
            "Receta copiada al portapapeles (el script sigue en la caché).",
            "Recette copiée dans le presse-papiers (le script reste dans le cache).",
        )
    }
    pub fn confirm_heading(&self) -> &'static str {
        self.get(
            "The following command will be executed:",
            "Folgender Befehl wird ausgeführt:",
            "Şu komut çalıştırılacak:",
            "Se ejecutará el siguiente comando:",
            "La commande suivante sera exécutée :",
        )
    }
    pub fn confirm_run(&self) -> &'static str {
        self.get(
            "Confirm and run",
            "Bestätigen und ausführen",
            "Onayla ve çalıştır",
            "Confirmar y ejecutar",
            "Confirmer et exécuter",
        )
    }
    pub fn cancel(&self) -> &'static str {
        self.get("Cancel", "Abbrechen", "İptal", "Cancelar", "Annuler")
    }
    pub fn open_folder_error(&self) -> &'static str {
        self.get(
            "Could not open folder: {}",
            "Ordner konnte nicht geöffnet werden: {}",
            "Klasör açılamadı: {}",
            "No se pudo abrir la carpeta: {}",
            "Impossible d'ouvrir le dossier : {}",
        )
    }
    pub fn status_host(&self) -> &'static str {
        self.get("Host:", "Host:", "Sunucu:", "Equipo:", "Hôte :")
    }
    pub fn status_id(&self) -> &'static str {
        self.get("ID:", "ID:", "Kimlik:", "ID:", "Identifiant :")
    }
    pub fn status_image(&self) -> &'static str {
        self.get(
            "Base image:",
            "Basis-Image:",
            "Temel görüntü:",
            "Imagen base:",
            "Image de base :",
        )
    }
    pub fn status_search(&self) -> &'static str {
        self.get("Search:", "Suche:", "Arama:", "Búsqueda:", "Recherche :")
    }
    pub fn status_engine(&self) -> &'static str {
        self.get("Engine:", "Engine:", "Motor:", "Motor:", "Moteur :")
    }
    pub fn status_step(&self) -> &'static str {
        self.get("Step:", "Schritt:", "Adım:", "Paso:", "Étape :")
    }
    pub fn progress_label(&self, percent: u32, step: &str) -> String {
        let name = self.get(
            "Progress",
            "Fortschritt",
            "İlerleme",
            "Progreso",
            "Progression",
        );
        if step.trim().is_empty() {
            format!("{}: {} %", name, percent)
        } else {
            format!("{}: {} % — {}", name, percent, step)
        }
    }
    pub fn status_language(&self) -> &'static str {
        self.get("Language:", "Sprache:", "Dil:", "Idioma:", "Langue :")
    }
    pub fn status_theme(&self) -> &'static str {
        self.get("Theme:", "Design:", "Tema:", "Tema:", "Thème :")
    }
    pub fn theme_auto(&self) -> &'static str {
        self.get("Auto", "Auto", "Otomatik", "Automático", "Auto")
    }
    pub fn theme_light(&self) -> &'static str {
        self.get("Light", "Hell", "Açık", "Claro", "Clair")
    }
    pub fn theme_dark(&self) -> &'static str {
        self.get("Dark", "Dunkel", "Koyu", "Oscuro", "Sombre")
    }
    pub fn theme_label(&self, mode: ThemeMode) -> &'static str {
        match mode {
            ThemeMode::Auto => self.theme_auto(),
            ThemeMode::Light => self.theme_light(),
            ThemeMode::Dark => self.theme_dark(),
        }
    }

    // ---------- Über / Info ----------

    pub fn info_button(&self) -> &'static str {
        self.get("Info", "Info", "Bilgi", "Info", "Info")
    }
    pub fn info_title(&self) -> &'static str {
        self.get(
            "About git2pkg",
            "Über git2pkg",
            "git2pkg hakkında",
            "Acerca de git2pkg",
            "À propos de git2pkg",
        )
    }
    pub fn about_description(&self) -> &'static str {
        self.get(
            "Builds Git repositories inside an isolated disposable container to keep your host system clean.",
            "Baut Git-Repositories isoliert in einem Wegwerf-Container, damit das Host-System sauber bleibt.",
            "Ana sisteminizi temiz tutmak için Git depolarını izole ve tek kullanımlık bir konteyner içinde derler.",
            "Compila repositorios Git dentro de un contenedor desechable aislado para mantener limpio tu sistema anfitrión.",
            "Compile des dépôts Git dans un conteneur jetable isolé afin de garder votre système hôte propre.",
        )
    }
    pub fn link_repository(&self) -> &'static str {
        self.get(
            "Source code / Repository",
            "Quellcode / Repository",
            "Kaynak kod / Depo",
            "Código fuente / Repositorio",
            "Code source / Dépôt",
        )
    }
    pub fn link_support(&self) -> &'static str {
        self.get(
            "♥ Support the project",
            "♥ Projekt unterstützen",
            "♥ Projeyi destekle",
            "♥ Apoyar el proyecto",
            "♥ Soutenir le projet",
        )
    }
    pub fn close(&self) -> &'static str {
        self.get("Close", "Schließen", "Kapat", "Cerrar", "Fermer")
    }

    // ---------- Build-Orchestrierung ----------

    pub fn step_workdir(&self) -> &'static str {
        self.get(
            "Creating working directory",
            "Arbeitsverzeichnis anlegen",
            "Çalışma dizini oluşturuluyor",
            "Creando directorio de trabajo",
            "Création du répertoire de travail",
        )
    }
    pub fn step_clone(&self) -> &'static str {
        self.get(
            "Cloning repository",
            "Repository klonen",
            "Depo klonlanıyor",
            "Clonando repositorio",
            "Clonage du dépôt",
        )
    }
    pub fn step_build_system(&self, name: &str) -> String {
        format!(
            "{}: {}",
            self.get(
                "Build system",
                "Build-System",
                "Derleme sistemi",
                "Sistema de compilación",
                "Système de compilation",
            ),
            name
        )
    }
    pub fn step_strategy_round(
        &self,
        strategy: &str,
        index: u8,
        total: u8,
        round: u8,
        max_rounds: u8,
    ) -> String {
        format!(
            "{} {}/{}: {} — {} {}/{}",
            self.get(
                "Strategy",
                "Strategie",
                "Strateji",
                "Estrategia",
                "Stratégie"
            ),
            index,
            total,
            strategy,
            self.get("Round", "Runde", "Tur", "Ronda", "Tour"),
            round,
            max_rounds
        )
    }

    #[allow(dead_code)]
    pub fn step_attempt(&self, current: u8, max: u8) -> String {
        format!(
            "{} {}/{}",
            self.get(
                "Build attempt",
                "Build-Versuch",
                "Derleme denemesi",
                "Intento de compilación",
                "Tentative de compilation",
            ),
            current,
            max
        )
    }
    pub fn step_search(&self, label: &str) -> String {
        format!(
            "{} {}",
            self.get(
                "Searching package for",
                "Suche Paket für",
                "Paket aranıyor:",
                "Buscando paquete para",
                "Recherche du paquet pour",
            ),
            label
        )
    }
    pub fn step_install(&self, package: &str) -> String {
        format!(
            "{}: {}",
            self.get(
                "Installing in container",
                "Nachinstallation im Container",
                "Kapsayıcıya kuruluyor",
                "Instalando en el contenedor",
                "Installation dans le conteneur",
            ),
            package
        )
    }
    pub fn step_copy_artifacts(&self) -> &'static str {
        self.get(
            "Copying artifacts to ./dist",
            "Artefakte nach ./dist kopieren",
            "Yapıtlar ./dist içine kopyalanıyor",
            "Copiando artefactos a ./dist",
            "Copie des artefacts vers ./dist",
        )
    }
    pub fn err_clone_failed(&self) -> &'static str {
        self.get(
            "git clone failed – check the URL.",
            "git clone ist fehlgeschlagen – URL prüfen.",
            "git clone başarısız – URL'yi kontrol edin.",
            "Error en git clone: compruebe la URL.",
            "Échec de git clone – vérifiez l'URL.",
        )
    }
    pub fn err_no_pattern(&self) -> &'static str {
        self.get(
            "Build failed; no known error pattern found. See the log.",
            "Build fehlgeschlagen; kein bekanntes Fehlermuster gefunden. Siehe Log.",
            "Derleme başarısız; bilinen bir hata deseni bulunamadı. Günlüğe bakın.",
            "Falló la compilación; no se encontró un patrón de error conocido. Vea el registro.",
            "Échec de la compilation ; aucun motif d'erreur connu. Consultez le journal.",
        )
    }
    pub fn err_no_package(&self, label: &str) -> String {
        format!(
            "{} {}",
            self.get(
                "No package found for",
                "Kein Paket gefunden für",
                "Paket bulunamadı:",
                "No se encontró paquete para",
                "Aucun paquet trouvé pour",
            ),
            label
        )
    }
    pub fn err_max_attempts(&self, max: u8, output: &str) -> String {
        let head = match self.lang {
            Language::En => format!("Build failed after {} attempts.", max),
            Language::De => format!("Build nach {} Versuchen fehlgeschlagen.", max),
            Language::Tr => format!("Derleme {} denemeden sonra başarısız oldu.", max),
            Language::Es => format!("Compilación fallida tras {} intentos.", max),
            Language::Fr => format!("Compilation échouée après {} tentatives.", max),
        };
        format!("{}\n{}", head, output)
    }
    pub fn log_setup(&self, manager: &str) -> String {
        format!(
            "== git2pkg: {} ({}) ==",
            self.get(
                "Setup",
                "Setup",
                "Kurulum",
                "Configuración",
                "Configuration"
            ),
            manager
        )
    }
    pub fn log_extra(&self, packages: &str) -> String {
        format!(
            "== git2pkg: {} {} ==",
            self.get(
                "Extra packages",
                "Nachinstallation",
                "Ek paketler",
                "Paquetes adicionales",
                "Paquets supplémentaires",
            ),
            packages
        )
    }
    pub fn log_binary_build(&self) -> &'static str {
        self.get(
            "== git2pkg: binary build ==",
            "== git2pkg: Binary-Build ==",
            "== git2pkg: İkili derleme ==",
            "== git2pkg: compilación binaria ==",
            "== git2pkg: compilation binaire ==",
        )
    }
    #[allow(dead_code)]
    pub fn log_pkgbuild(&self) -> &'static str {
        "== git2pkg: PKGBUILD/makepkg =="
    }
    #[allow(dead_code)]
    pub fn log_deb(&self) -> &'static str {
        "== git2pkg: dpkg-deb =="
    }
    #[allow(dead_code)]
    pub fn log_rpm(&self) -> &'static str {
        "== git2pkg: rpmbuild =="
    }
    pub fn unknown_system(&self) -> &'static str {
        self.get(
            "No known build system detected – trying a generic build.",
            "Kein bekanntes Build-System erkannt – generischer Versuch.",
            "Bilinen derleme sistemi yok – genel derleme deneniyor.",
            "No se detectó un sistema de compilación conocido – se intenta una compilación genérica.",
            "Aucun système de compilation connu détecté – tentative de compilation générique.",
        )
    }
    pub fn err_no_artifacts(&self) -> &'static str {
        self.get(
            "Build reported success but produced no artifacts.",
            "Build meldete Erfolg, aber es wurden keine Artefakte erzeugt.",
            "Derleme başarılı bildirdi ancak yapıt üretmedi.",
            "La compilación terminó correctamente pero no produjo artefactos.",
            "La compilation a réussi mais n'a produit aucun artefacte.",
        )
    }
    pub fn cli_header(&self, engine: &str, distro: &str) -> String {
        format!("git2pkg CLI – {} – {}", engine, distro)
    }
    pub fn cli_done(&self) -> &'static str {
        self.get(
            "==> Build successful. Artifacts in ./dist/:",
            "==> Build erfolgreich. Artefakte in ./dist/:",
            "==> Derleme başarılı. Yapıtlar ./dist/ içinde:",
            "==> Compilación correcta. Artefactos en ./dist/:",
            "==> Compilation réussie. Artefacts dans ./dist/ :",
        )
    }
    pub fn cli_install(&self, command: &str) -> String {
        format!(
            "{} {}",
            self.get(
                "Install:",
                "Installieren:",
                "Kurulum:",
                "Instalar:",
                "Installer :"
            ),
            command
        )
    }
    pub fn cli_failed(&self, error: &str) -> String {
        format!(
            "{} {}",
            self.get(
                "==> Error:",
                "==> Fehler:",
                "==> Hata:",
                "==> Error:",
                "==> Erreur :"
            ),
            error
        )
    }

    pub fn install_success_title(&self) -> &'static str {
        self.get(
            "Package built successfully",
            "Paket erfolgreich gebaut",
            "Paket başarıyla derlendi",
            "Paquete compilado correctamente",
            "Paquet compilé avec succès",
        )
    }
    pub fn install_success_body(&self) -> &'static str {
        self.get(
            "Built package:",
            "Fertiges Paket:",
            "Derlenen paket:",
            "Paquete compilado:",
            "Paquet compilé :",
        )
    }
    pub fn install_question(&self) -> &'static str {
        self.get(
            "Do you want to install this package on your system now?",
            "Möchtest du dieses Paket jetzt auf deinem System installieren?",
            "Bu paketi şimdi sistemine kurmak istiyor musun?",
            "¿Quieres instalar este paquete en tu sistema ahora?",
            "Voulez-vous installer ce paquet sur votre système maintenant ?",
        )
    }
    pub fn install_now(&self) -> &'static str {
        self.get(
            "Install now",
            "Jetzt installieren",
            "Şimdi kur",
            "Instalar ahora",
            "Installer maintenant",
        )
    }
    pub fn install_later(&self) -> &'static str {
        self.get(
            "Later / Close",
            "Später / Schließen",
            "Sonra / Kapat",
            "Más tarde / Cerrar",
            "Plus tard / Fermer",
        )
    }
    pub fn install_running(&self) -> &'static str {
        self.get(
            "Starting installation …",
            "Installation wird gestartet …",
            "Kurulum başlatılıyor …",
            "Iniciando instalación …",
            "Lancement de l’installation …",
        )
    }
    pub fn install_ok(&self) -> &'static str {
        self.get(
            "Installation successful.",
            "Installation erfolgreich.",
            "Kurulum başarılı.",
            "Instalación correcta.",
            "Installation réussie.",
        )
    }
    pub fn install_failed(&self) -> &'static str {
        self.get(
            "Installation failed:",
            "Installation fehlgeschlagen:",
            "Kurulum başarısız:",
            "Error en la instalación:",
            "Échec de l’installation :",
        )
    }
    pub fn cleanup_title(&self) -> &'static str {
        self.get(
            "Clean up build cache?",
            "Build-Cache aufräumen?",
            "Derleme önbelleğini temizle?",
            "¿Limpiar la caché de compilación?",
            "Nettoyer le cache de compilation ?",
        )
    }
    pub fn cleanup_body(&self) -> &'static str {
        self.get(
            "The last build failed. Do you want to clean up the temporary build and source files in the cache before git2pkg exits?",
            "Der letzte Build ist fehlgeschlagen. Möchtest du die temporären Build- und Quellcode-Dateien im Cache aufräumen, bevor git2pkg beendet wird?",
            "Son derleme başarısız oldu. git2pkg kapanmadan önce önbellekteki geçici derleme ve kaynak kod dosyalarını temizlemek istiyor musun?",
            "La última compilación falló. ¿Quieres limpiar los archivos temporales de compilación y el código fuente en la caché antes de que se cierre git2pkg?",
            "La dernière compilation a échoué. Voulez-vous nettoyer les fichiers temporaires de compilation et les sources dans le cache avant que git2pkg ne se ferme ?",
        )
    }

    pub fn retry_failed_title(&self) -> &'static str {
        self.get(
            "Build failed",
            "Build fehlgeschlagen",
            "Derleme başarısız",
            "Compilación fallida",
            "Compilation échouée",
        )
    }
    pub fn retry_failed_question(&self) -> &'static str {
        self.get(
            "Do you want to retry using the cached source and an alternative build strategy?",
            "Möchtest du den Build mit dem vorhandenen Quellcode-Cache und einer alternativen Strategie erneut versuchen?",
            "Mevcut kaynak kodu önbelleği ve alternatif bir stratejiyle derlemeyi yeniden denemek istiyor musun?",
            "¿Quieres reintentar usando la caché de código fuente y una estrategia alternativa?",
            "Voulez-vous réessayer avec le cache des sources et une autre stratégie ?",
        )
    }
    pub fn retry_failed_yes(&self) -> &'static str {
        self.get(
            "Yes, try another strategy",
            "Ja, mit anderer Strategie versuchen",
            "Evet, farklı bir stratejiyle dene",
            "Sí, probar con otra estrategia",
            "Oui, essayer une autre stratégie",
        )
    }
    pub fn retry_failed_cancel(&self) -> &'static str {
        self.get(
            "Cancel / Check log",
            "Abbrechen / Log prüfen",
            "İptal / Günlüğü kontrol et",
            "Cancelar / Revisar registro",
            "Annuler / Voir le journal",
        )
    }

    pub fn learned_rules(&self) -> &'static str {
        self.get(
            "Learned rules:",
            "Gelernte Regeln:",
            "Öğrenilen kurallar:",
            "Reglas aprendidas:",
            "Règles apprises :",
        )
    }

    pub fn cleanup_delete_all(&self) -> &'static str {
        self.get(
            "Delete everything & exit",
            "Alles löschen & beenden",
            "Her şeyi sil ve çık",
            "Eliminar todo y salir",
            "Tout supprimer et quitter",
        )
    }
    pub fn cleanup_delete_artifacts(&self) -> &'static str {
        self.get(
            "Delete build artifacts only & exit",
            "Nur Build-Reste löschen & beenden",
            "Sadece derleme kalıntılarını sil ve çık",
            "Eliminar solo restos de compilación y salir",
            "Supprimer uniquement les artefacts de compilation et quitter",
        )
    }
    pub fn cleanup_delete_nothing(&self) -> &'static str {
        self.get(
            "Delete nothing & exit",
            "Nichts löschen & beenden",
            "Hiçbir şey silme ve çık",
            "No eliminar nada y salir",
            "Ne rien supprimer et quitter",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alle_sprachen_sind_erreichbar() {
        assert_eq!(Language::ALL.len(), 5);
        assert_eq!(Language::En.label(), "EN");
        assert_eq!(Language::De.label(), "DE");
        assert_eq!(Language::Tr.label(), "TR");
        assert_eq!(Language::Es.label(), "ES");
        assert_eq!(Language::Fr.label(), "FR");
    }

    #[test]
    fn retry_button_ist_in_fuenf_sprachen_vorhanden() {
        let de = Texts::new(Language::De).retry_alternative();
        assert_eq!(de, "Erneut versuchen (Andere Strategie)");
        let mut seen = std::collections::HashSet::new();
        for lang in Language::ALL {
            let text = Texts::new(lang).retry_alternative();
            assert!(!text.is_empty(), "leerer Retry-Text für {:?}", lang);
            assert!(seen.insert(text), "doppelter Retry-Text für {:?}", lang);
        }
    }

    #[test]
    fn thema_wechselt_zyklisch() {
        assert_eq!(ThemeMode::Auto.next(), ThemeMode::Light);
        assert_eq!(ThemeMode::Light.next(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Dark.next(), ThemeMode::Auto);
    }

    #[test]
    fn texte_sind_in_fuenf_sprachen_verschieden() {
        let mut seen = std::collections::HashSet::new();
        for lang in Language::ALL {
            let texts = Texts::new(lang);
            assert!(
                seen.insert(texts.build_start()),
                "doppelter Text für {:?}",
                lang
            );
        }
        assert_eq!(Texts::new(Language::De).build_start(), "Build starten");
        assert_eq!(Texts::new(Language::En).build_start(), "Start build");
        assert_eq!(Texts::new(Language::Tr).build_start(), "Derlemeyi başlat");
        assert_eq!(
            Texts::new(Language::Es).build_start(),
            "Iniciar compilación"
        );
        assert_eq!(
            Texts::new(Language::Fr).build_start(),
            "Lancer la compilation"
        );
    }

    #[test]
    fn cross_build_warning_ist_in_fuenf_sprachen_vorhanden() {
        for lang in Language::ALL {
            let text = Texts::new(lang).cross_build_warning();
            assert!(text.len() > 80, "zu kurz für {:?}: {}", lang, text);
        }
        assert!(Texts::new(Language::De)
            .cross_build_warning()
            .starts_with("Hinweis:"));
        assert!(Texts::new(Language::En)
            .cross_build_warning()
            .starts_with("Note:"));
        assert!(Texts::new(Language::Es)
            .cross_build_warning()
            .starts_with("Nota:"));
        assert_eq!(Texts::new(Language::De).ok_understood(), "OK, verstanden");
        assert_eq!(Texts::new(Language::En).ok_understood(), "OK, got it");
        assert_eq!(Texts::new(Language::Tr).ok_understood(), "Tamam, anladım");
        assert_eq!(Texts::new(Language::Es).ok_understood(), "Entendido");
        assert_eq!(Texts::new(Language::Fr).ok_understood(), "OK, compris");
        assert_eq!(Texts::new(Language::En).clear_url(), "Clear URL");
        assert_eq!(Texts::new(Language::De).clear_url(), "URL löschen");
        assert_eq!(Texts::new(Language::Tr).clear_url(), "URL'yi temizle");
        assert_eq!(Texts::new(Language::Es).clear_url(), "Borrar URL");
        assert_eq!(Texts::new(Language::Fr).clear_url(), "Effacer l'URL");
        assert_eq!(Texts::new(Language::En).save_log(), "Save log");
        assert_eq!(Texts::new(Language::De).save_log(), "Log speichern");
        assert_eq!(Texts::new(Language::Tr).save_log(), "Günlüğü kaydet");
        assert_eq!(Texts::new(Language::Es).save_log(), "Guardar registro");
        assert_eq!(
            Texts::new(Language::Fr).save_log(),
            "Enregistrer le journal"
        );
    }

    #[test]
    fn url_kontextmenue_ist_in_allen_sprachen_vorhanden() {
        let paste = [
            Texts::new(Language::En).context_paste(),
            Texts::new(Language::De).context_paste(),
            Texts::new(Language::Tr).context_paste(),
            Texts::new(Language::Es).context_paste(),
            Texts::new(Language::Fr).context_paste(),
        ];
        assert_eq!(paste, ["Paste", "Einfügen", "Yapıştır", "Pegar", "Coller"]);
        let cut = [
            Texts::new(Language::En).context_cut(),
            Texts::new(Language::De).context_cut(),
            Texts::new(Language::Tr).context_cut(),
            Texts::new(Language::Es).context_cut(),
            Texts::new(Language::Fr).context_cut(),
        ];
        assert_eq!(cut, ["Cut", "Ausschneiden", "Kes", "Cortar", "Couper"]);
    }

    #[test]
    fn tuerkische_sonderzeichen_sind_vorhanden() {
        let tr = Texts::new(Language::Tr);
        let build_system = tr.step_build_system("Cargo (Rust)");
        let combined = [
            tr.subtitle(),
            tr.repo_url(),
            tr.build_start(),
            tr.starting(),
            tr.success_step(),
            tr.cancel(),
            tr.copy_log(),
            tr.confirm_run(),
            tr.confirm_heading(),
            tr.step_workdir(),
            &build_system,
            tr.step_clone(),
            tr.log_binary_build(),
            tr.install_package(),
            tr.open_dir(),
        ]
        .concat();
        // Das grosse Ğ steht in natürlichen türkischen Wörtern kaum am Anfang;
        // seine Glyphenabdeckung wurde im eingebetteten Font geprüft.
        for c in ['İ', 'ı', 'ğ', 'ş', 'Ş', 'ç', 'ö', 'ü'] {
            assert!(combined.contains(c), "fehlendes Zeichen: {}", c);
        }
    }

    #[test]
    fn strategie_und_runde_werden_angezeigt() {
        let text = Texts::new(Language::De).step_strategy_round("System-Bibliotheken", 2, 4, 1, 10);
        assert!(text.contains("Strategie 2/4"), "{}", text);
        assert!(text.contains("System-Bibliotheken"), "{}", text);
        assert!(text.contains("Runde 1/10"), "{}", text);
    }

    #[test]
    fn fortschritt_ist_im_ruhezustand_lokalisiert() {
        let de = Texts::new(Language::De).progress_label(0, "");
        assert_eq!(de, "Fortschritt: 0 %");
        let en = Texts::new(Language::En).progress_label(42, "Building");
        assert_eq!(en, "Progress: 42 % — Building");
    }

    #[test]
    fn fallback_ist_hell() {
        assert!(!ThemeMode::Light.is_dark());
        assert!(ThemeMode::Dark.is_dark());
    }
}
