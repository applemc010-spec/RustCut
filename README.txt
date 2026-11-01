RustCut — محرر فيديو (Rust + Tauri + FFmpeg)
==========================================

الطريقة 1 (بدون تثبيت أي شي على جهازك): بناء الـ exe تلقائياً بالسحابة
1. سوّي حساب مجاني على github.com وأنشئ مستودع (Repository) جديد.
2. ارفع كل محتويات هذا المجلد إلى المستودع (Add file > Upload files).
   مهم: لازم يظهر مجلد .github (إذا ما ظهر، فعّل Show hidden files بالويندوز قبل الرفع).
3. افتح تبويب Actions > build-windows > Run workflow.
4. بعد 10-20 دقيقة، افتح التشغيل المكتمل ونزّل RustCut-installer من قسم Artifacts.
5. فك الضغط وشغّل ملف الـ setup.exe. (ويندوز ممكن يطلع تحذير SmartScreen لأن البرنامج غير موقّع: More info > Run anyway)

الطريقة 2: البناء على جهازك
1. ثبّت: Rust (rustup.rs) + Node.js 20 + Visual Studio Build Tools (Desktop development with C++).
2. نزّل ffmpeg.exe (مثلاً من github.com/BtbN/FFmpeg-Builds) وضعه بالمسار: src-tauri\bin\ffmpeg.exe
3. بالـ PowerShell داخل هذا المجلد:
     npm install
     npx tauri icon app-icon.png
     npx tauri build
4. الملف الجاهز: src-tauri\target\release\bundle\nsis\*.exe

للتجربة السريعة بدون بناء نهائي:  npx tauri dev

المختصرات: Space تشغيل | S تقسيم | + / ] تسريع | - / [ تبطيء | Delete حذف | Ctrl+Z تراجع | F1 كل المختصرات
