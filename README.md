# OhMyLock

Hafif, çapraz platform (Windows / macOS / Linux) çalışan bir menü çubuğu
(tray) uygulaması: simgesinden "Kilitle" seçildiğinde tam ekran bir kilit
ekranı açılır; yalnızca daha önce kaydedilen yüzün webcam ile tekrar
tanınmasıyla kapanır, sonra uygulama tekrar tepsi/menü çubuğunda bekler ve
istediğin zaman yeniden kilitleyebilirsin. Rust ile yazıldı: native binary,
Chromium/Electron gibi bir runtime taşımaz.

Ölçülen ayak izi: tepsi ikonu boşta beklerken (kamera/model henüz
yüklenmemişken) **~70 MB RAM, ~%0 CPU**. Kilitliyken kamera 640x480 @ 2 Hz
örneklenir ve yalnızca o sırada model belleğe yüklenir.

## Güvenlik tasarımı — önemli

- Kilit ekranı normal UI yollarıyla (kapatma düğmesi, Alt+F4, pencere
  dışına tıklama) **kasıtlı olarak** kapatılamaz; yalnızca doğrulanmış bir
  yüz eşleşmesiyle kapanır.
- Kilit aktifken tepsi menüsündeki "Çıkış" da kasıtlı olarak devre dışıdır
  (aksi halde uygulamayı kapatmak kilidi atlatmanın bir yolu olurdu).
  "Çıkış" yalnızca kilit açıkken çalışır.
- İşletim sisteminin kendi süreç yönetimi (Görev Yöneticisi / `taskkill`,
  Activity Monitor, `kill`, güvenli mod) her zaman geçerli bir kapatma
  yoludur ve bu uygulama bunu engellemeye **çalışmaz**. Bu kasıtlıdır:
  kilitlenme durumunda (kamera arızası, model dosyası bozulması vb.)
  kendi kendine kilitlenmeni önlemenin tek güvenli yoludur.

### Canlılık tespiti (anti-spoof)

Sadece embedding eşleşmesi, kayıtlı kişinin **basılı bir fotoğrafı** veya
**telefon ekranında açılmış bir görseli** kameraya tutulduğunda da
eşleşir -- ArcFace/MobileFaceNet embedding'i "bu görüntüdeki yüz kime ait"
sorusunu cevaplar, "bu gerçek bir yüz mü" sorusunu değil. Bunu kapatmak
için, embedding eşleştikten *sonra* ayrı bir MiniFASNetV2 (`2.7_80x80`)
canlılık sınıflandırıcısı çalışır ve üç sınıf üretir: gerçek yüz / basılı
fotoğraf saldırısı / ekran tekrar saldırısı. Canlılık skoru eşiğin
(varsayılan 0.5) altındaysa ekran kilitli kalır ve "Fotoğraf/ekran
algılandı" mesajı gösterilir -- embedding eşleşse bile.

Bu, olası her saldırıyı engelleyen kesin bir güvenlik garantisi **değildir**
(örn. yüksek kaliteli video tekrarı veya 3D maske gibi daha sofistike
yöntemlere karşı test edilmemiştir); amaç, en yaygın fiziksel atlatma
yolunu -- kameraya bir fotoğraf tutmak -- kapatmaktır.

**Eşik değeri gerçek bir kamerayla doğrulanmamıştır** -- modelin kendi
dokümantasyonundan alınmış bir başlangıç noktasıdır, ölçülmüş bir değer
değil. Tüketici bir webcam'in otomatik beyaz dengesi/pozlaması ve MJPEG
sıkıştırması, gerçek bir yüzün skorunu küratörlü bir referans setinden
beklenenden daha düşük çekebilir; gerçek bir yüz yanlışlıkla reddediliyorsa
muhtemelen budur. Her reddedilişte gerçek skor stderr'e yazılır
(`antispoof: rejected candidate match (score=...)`), böylece gerçek
verilerle yeniden ayarlanabilir. Yanlış pozitifler devam ederse, Ayarlar
penceresindeki "Canlılık tespiti" anahtarından tamamen kapatılabilir.

### Yüz eşleştirme doğruluğu (önemli bir düzeltme)

İlk sürümde büyük bir ArcFace-ResNet100 modeli (261 MB) hizalanmamış
(ham bounding-box kırpması) yüz görüntüleriyle besleniyordu. Bu, modelin
eğitim dağılımının tamamen dışına çıktığı için embedding'lerin ayırt
ediciliğini kaybetmesine ve **pratikte hemen hemen her yüzün eşleşmiş gibi
görünmesine** yol açıyordu. Kök neden ile ilgili detaylar:

- ArcFace ailesi modeller yalnızca 5-nokta yüz landmark'ı (gözler, burun,
  ağız köşeleri) ile **hizalanmış** 112x112 kırpmalarla anlamlı çalışır.
- Şimdiki sürüm, tespit ve 5-nokta landmark'ı tek geçişte veren SCRFD
  (`det_500m`, ~2.5 MB) modelini kullanıyor, landmark'lardan kapalı-form bir
  benzerlik dönüşümü (ölçek+dönüş+öteleme) hesaplayıp yüzü standart ArcFace
  şablonuna hizalıyor, ardından MobileFaceNet (`w600k_mbf`, ~13.6 MB) ile
  512 boyutlu embedding çıkarıyor.
- Bu düzeltme gerçek fotoğraflarla doğrulandı: **farklı iki kişi arasında
  kosinüs benzerliği ≈ -0.15**, **aynı kişinin döndürülmüş/yeniden
  boyutlandırılmış farklı bir fotoğrafında ≈ 0.95** ölçüldü (varsayılan
  eşik 0.5). `examples/face_match_test.rs` bu testi tekrarlamak için
  kullanılabilir (kamerasız, iki resim dosyası vererek).

## Kurulum

### 1. Rust toolchain

https://rustup.rs üzerinden kurulur (Windows/macOS/Linux hepsinde aynı).

### 2. Sistem bağımlılıkları

- **Linux**: `v4l-utils` / `libv4l-dev` (kamera) ve tepsi ikonu için
  `libappindicator3` veya `libayatana-appindicator3` (masaüstü ortamına
  göre); `libxdo-dev`.
- **macOS**: Ekstra kurulum gerekmez. İlk kilitlemede Sistem Ayarları →
  Gizlilik ve Güvenlik → Kamera'dan uygulamayı çalıştırdığın terminale
  (Terminal.app / iTerm / vb.) kamera izni vermen istenecek.
- **Windows**: Ekstra kurulum gerekmez (Media Foundation kullanılır).

### 3. ONNX modellerini indir

Bu depo model ağırlıklarını içermez (ikili dosyalar). Üç küçük model
gerekir:

1. **Yüz tespiti + landmark** — `det_500m.onnx` (~2.5 MB, InsightFace
   `buffalo_s`)
2. **Yüz embedding** — `w600k_mbf.onnx` (~13.6 MB, MobileFaceNet,
   InsightFace `buffalo_s`)
3. **Canlılık tespiti (anti-spoof)** — `minifasnet_v2.onnx` (~1.7 MB,
   MiniFASNetV2 2.7_80x80)

İlk ikisi şu adresten indirilebilir:
https://huggingface.co/deepghs/insightface/tree/main/buffalo_s

Üçüncüsü şu adresten (Apache-2.0, minivision-ai'nin
`2.7_80x80_MiniFASNetV2.pth` ağırlığının bit-eşdeğer ONNX dönüşümü):
https://huggingface.co/garciafido/minifasnet-v2-anti-spoofing-onnx

İlk iki model ağırlığı InsightFace projesine ait ve kendi lisans
koşullarına tabidir (bu depodaki Apache-2.0 lisansının kapsamı dışında) —
kullanmadan önce InsightFace'in lisansını kontrol et, özellikle ticari
kullanım düşünüyorsan.

İndirdiğin dosyaları şu adlarla, uygulamanın veri dizinine koy:

- macOS: `~/Library/Application Support/dev.facelock.FaceLock/models/`
- Linux: `~/.local/share/facelock/models/`
- Windows: `%APPDATA%\facelock\facelock\models\`

Dosya adları: `det_500m.onnx` → `detector.onnx`, `w600k_mbf.onnx` →
`embedder.onnx`, `minifasnet_v2.onnx` → `antispoof.onnx` olarak yeniden
adlandır (veya `--detector-model` / `--embedder-model` /
`--antispoof-model` bayraklarıyla farklı bir yol/isim ver).

## Kullanım

```bash
# 1) Kayıt: kameraya bak, birkaç örnek yüz gömmesi alınır ve ortalaması kaydedilir
cargo run --release -- --enroll

# 2) Tepsi/menü çubuğu uygulamasını başlat
cargo run --release
```

Uygulama açılınca menü çubuğunda/tepside bir görüntüleyici-çerçeve/yüz
simgesi belirir (Dock'ta veya görev çubuğunda görünmez). "Kilitle"
seçildiğinde tam ekran kilit ekranı açılır: mavi = tarama, kırmızı =
eşleşmedi, turuncu = embedding eşleşti ama canlılık testi başarısız (bkz.
"Canlılık tespiti" yukarıda), yeşil = eşleşti (kilit birazdan kalkacak).
Kayıtlı yüz kamerada tekrar görülüp canlılık testini geçince ekran otomatik
kapanır ve uygulama tepside beklemeye devam eder — istediğin zaman tekrar
"Kilitle" diyebilirsin. "Çıkış" yalnızca kilit kapalıyken çalışır.

Tepsi menüsündeki "Ayarlar…" özel bir ayarlar penceresi açar (kilit
ekranıyla aynı grafit/lacivert görsel dilde):

- **Açılışta Başlat** — `~/Library/LaunchAgents/` içine bir LaunchAgent
  plist'i yazıp `launchctl` ile yükleyerek (macOS) uygulamayı oturum
  açılışında otomatik başlatır; kapatınca plist silinir ve ajan boşaltılır.
- **Canlılık tespiti** — yukarıdaki anti-spoof kontrolünün açma/kapama
  anahtarı.
- **Yüz görünmediğinde kilitle** — kilit açıkken kamerayı seyrek (4 sn'de
  bir) örnekler; ~20 saniye kimse görünmezse ekranı otomatik kilitler
  (masadan kalkıp uzaklaşmayı unutma senaryosu için). **Varsayılan kapalı**:
  açıkken kamera ve yüz modelleri kilit açıkken de yüklü kalır, bu da
  uygulamanın normal ~%0 boşta CPU/70 MB RAM ayak izinden ödün verir --
  bkz. "Performans notları".
- **Yüzü Yeniden Tanımla** — kayıtlı yüzü, terminale dönmeden doğrudan bu
  pencereden yeniden kaydeder (8 örnek kamerada taranır, ilerleme
  düğmenin üzerinde gösterilir).

## Performans notları

- Kamera 640x480 çözünürlükte, 2 Hz (500 ms aralıklarla) örneklenir.
- Kilit ekranı kapalıyken (tepside beklerken) kamera ve modeller belleğe
  yüklenmez; hiçbir periyodik yeniden çizim yapılmaz (tamamen olay
  güdümlü, ~%0 CPU) -- **"Yüz görünmediğinde kilitle" kapalıyken**.
  Açıldığında, kilit açık durumdayken de kamera 4 sn'de bir örneklenir ve
  yüz modelleri belleğe yüklü kalır.
- `cargo build --release`, LTO + `codegen-units=1` + `strip` ile derlenir.
- ONNX Runtime CPU execution provider kullanır (GPU gerekmez).

## Yeniden kayıt

Yüzünü değiştirmek/yeniden kaydetmek için iki yol var: `--enroll`
bayrağını tekrar çalıştırmak, ya da uygulama çalışırken tepsi menüsünden
"Ayarlar…" → "Yüzü Yeniden Tanımla". İkisi de önceki kayıt üzerine yazar.

## macOS: uygulama paketi ve simgeler

`cargo run` ile çalıştırılan çıplak binary de Dock'ta/Cmd+Tab'da görünmez
(uygulama başlarken `ActivationPolicy::Accessory` ile kendini arka plan
uygulaması olarak işaretler), ama Finder'da/Get Info'da gerçek bir simge
göstermek ve `LSUIElement` gibi paket düzeyinde ayarları taşımak için
gerçek bir `.app` paketi üretilebilir:

```bash
# Simgeyi (yeniden) üretmek istersen -- assets/AppIcon.png yazar:
cargo run --release --example gen_app_icon

# dist/OhMyLock.app paketini derler ve oluşturur:
./scripts/package_macos.sh
open dist/OhMyLock.app
```

Not: paketlenmiş uygulamanın bundle kimliği çıplak binary'den farklı
algılandığından macOS kamera iznini ilk açılışta tekrar soracaktır.

Menü çubuğu simgesi artık bir asma kilit değil, bir "yüz tarama"
göstergesi (köşe parantezleri + yüz halkası) -- kilit motifi tam ekran
kilit ekranına özel bırakıldı. `cargo run --release --example
preview_tray_icon` bu simgeyi `assets/tray_icon_preview.png` olarak
büyütülmüş halde kaydeder, tasarımda değişiklik yaparken hızlı önizleme
için kullanılabilir.

## Lisans

Apache License 2.0 -- bkz. [LICENSE](LICENSE). ONNX model ağırlıkları bu
kapsamda değildir, bkz. yukarıdaki "ONNX modellerini indir" bölümü. Kilit
ekranındaki metinler için gömülü DejaVu Sans fontu
(`assets/fonts/DejaVuSans.ttf`) ayrı ve izin verici bir lisansla dağıtılır,
bkz. `assets/fonts/LICENSE_DEJAVU.txt`.
