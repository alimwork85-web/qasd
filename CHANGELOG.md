# Changelog / سجل التغييرات

Qasd — by **Ali Alhumedi**. Built on [fsearch](https://github.com/noahdunnagan/fsearch) (MIT).

---

## v0.7.1 — 2026-10-15

### بالعربية

**المشاكل التي حللناها:**

1. **نافذة سوداء تظهر عند تشغيل الكمبيوتر**
   كانت مهمة تسجيل الدخول تشغّل خدمة الفهرسة `qasd.exe` مباشرة. ولأنه برنامج طرفية، كان ويندوز يفتح له نافذة سوداء تعرض سجلّ الفهرسة وتبقى مفتوحة. صارت المهمة تشغّل `qasdw.exe --daemon`، وهو برنامج لا يفتح نافذة طرفية: يشغّل الخدمة مخفية بصلاحيات المدير نفسها ثم يُغلق. تتم الأرشفة الآن في الخلفية دون أي نافذة، ويُكتب سجلّها في `%LOCALAPPDATA%\Qasd\daemon.log`.

2. **توقّف الخدمة تلقائياً بعد 3 أيام**
   المهمة المجدولة في ويندوز تُوقف ما تشغّله بعد 72 ساعة افتراضياً، فكانت الخدمة ستتوقف لو بقي الكمبيوتر يعمل 3 أيام متواصلة. الآن تنتهي المهمة نفسها فوراً بعد تشغيل الخدمة، فلا ينطبق عليها هذا الحد.

**التحديث:** ثبّت الإصدار الجديد بـ `install.cmd` كالعادة، فتُحدَّث مهمة تسجيل الدخول تلقائياً.

**أخرى:** صورة تعريفية ورسم بياني لسرعة الفهرسة في صفحة المشروع.

### English

**Fixed:**

1. **A black console window opened at sign-in.**
   The sign-in task ran the daemon, `qasd.exe serve`, directly; it's a console program, so Windows gave it a console window that showed the indexing log and stayed open. The task now runs `qasdw.exe --daemon`, a window-subsystem program that starts the daemon hidden, with the same administrator rights, and exits. Indexing runs in the background with no window; its log goes to `%LOCALAPPDATA%\Qasd\daemon.log`.

2. **The daemon would have been stopped after 3 days.**
   Task Scheduler ends what a task runs after 72 hours by default. The task now ends right after starting the daemon, so that limit no longer applies to it.

**Updating:** install the new version with `install.cmd` as usual; the sign-in task is updated automatically.

**Also:** a banner and an indexing-speed chart on the project page.

---

## v0.7.0 — 2026-10-15

### بالعربية

**الإضافات الجديدة:**

1. **قائمة الزر الأيمن الكاملة في نافذة البحث**
   النقر بالزر الأيمن على أي نتيجة (أو زر القائمة في لوحة المفاتيح، أو Shift+F10) يعرض قائمة الملف الحقيقية نفسها التي تظهر في مستكشف ويندوز: فتح باستخدام، إرسال إلى، قص، نسخ، إنشاء اختصار، حذف، خصائص، وإضافات البرامج المثبتة عندك (مثل NanaZip و7-Zip). وفي أعلاها ثلاثة عناصر خاصة بقصد: "فتح موقع الملف"، و"نسخ المسار"، و"ما الذي يشغل المساحة؟" للمجلدات. وShift مع الزر الأيمن يعرض القائمة الموسّعة كما في المستكشف.

2. **دعم الأقراص الخارجية (USB وبطاقات الذاكرة)**
   صارت الأقراص الخارجية تُفهرس تلقائياً لحظة توصيلها، خلال نحو ربع ثانية، وتختفي من النتائج لحظة فصلها. ويتابع قصد التغييرات عليها مثل الأقراص الداخلية. وعندما تطلب "إخراج الجهاز بأمان" يترك قصد القرص فوراً، فلا تظهر رسالة "الجهاز قيد الاستخدام". وإذا كنت تشغّله بصلاحيات المدير يُقرأ القرص الخارجي من جدول ملفاته (MFT) أيضاً.

3. **السحب والإفلات**
   اسحب أي نتيجة وأفلتها في أي برنامج: رسالة بريد، أو محرر، أو محادثة، أو مجلد. الملف يُسحب كما يُسحب من المستكشف تماماً، نسخاً أو نقلاً أو اختصاراً.

4. **لوحة معاينة الملف**
   لوحة على يمين النتائج تعرض الصورة المصغّرة الحقيقية للملف المحدد (صور، فيديو، PDF، مجلدات)، أو أول أسطر الملفات النصية والبرمجية بخط ثابت العرض، أو أيقونته، ومعها الاسم والنوع والحجم والتاريخ. تُحضَّر المعاينة في خيط مستقل، فلا تبطئ الكتابة. ولا يُنزَّل أي ملف موجود في سحابة OneDrive فقط من أجل معاينته.

5. **زرّا إظهار المعاينة وإخفائها**
   زر ✕ في زاوية لوحة المعاينة لإغلاقها، وزر داخل مربع البحث لإعادتها، يُضاء بالأزرق ما دامت ظاهرة. ويعمل Ctrl+P أيضاً، ويتذكر البرنامج اختيارك.

6. **سجل عمليات البحث السابقة**
   عندما يكون مربع البحث فارغاً تظهر آخر عمليات بحثك (حتى 30) بأيقونة ساعة. Enter يعيد البحث، وDelete يحذفه من السجل. ويُحفظ البحث فقط عندما تستفيد من نتائجه، أي حين تفتح أو تنسخ أو تسحب نتيجة، لا عند كل حرف تكتبه.

7. **حجم كل ملف وتاريخه، وحجم المجلدات كاملة**
   تعرض كل نتيجة حجمها وتاريخ آخر تعديل. وحجم المجلد يشمل كل ما بداخله على أي عمق، ويُحسب من الفهرس مباشرة دون قراءة القرص، ويطابق حساب PowerShell.

8. **ما الذي يشغل مساحة القرص؟** (بديل Filelight)
   افتحه بـ Ctrl+U، أو بزر الدائرة في مربع البحث، أو بالزر الأيمن على مجلد. يعرض محتويات المجلد من الأكبر إلى الأصغر، مع الحجم والنسبة وشريط يوضح نصيب كل عنصر. Enter يدخل المجلد، وBackspace يصعد مستوى، وEsc يعود إلى البحث. ويُحسب قرص C: كاملاً (مليونا ملف) في نحو 8 أجزاء من الألف من الثانية، من الفهرس دون قراءة القرص، فلا تتأثر سرعة البحث.

9. **اسم المطوّر وحقوقه**
   صار اسم المطوّر **Ali Alhumedi** في خصائص الملف (الشركة وحقوق النشر والمنتج والإصدار). وصار قصد يظهر في "التطبيقات المثبتة" في ويندوز وبرامج إزالة التثبيت، والناشر فيها Ali Alhumedi، ومعه الإصدار والأيقونة والحجم وتاريخ التثبيت.

10. **إزالة تثبيت كاملة من إعدادات ويندوز**
    زر "إلغاء التثبيت" في التطبيقات المثبتة يزيل كل شيء: البرنامج، ومهمة التشغيل عند الدخول، والتشغيل مع ويندوز، وإدخال PATH، والفهرس والإعدادات. ويطلب صلاحيات المدير مرة واحدة إن احتاجها.

11. **خيارات جديدة للنافذة**
    `qasdw --usage [مجلد]` يفتح النافذة مباشرة على عرض المساحة، ويفيد في عمل اختصار على سطح المكتب.

**المشاكل التي حللناها:**

1. **المعاينة كانت تعرض أيقونة نوع الملف بدل الصورة نفسها**
   إذا طُلبت صورة مصغّرة أكبر من الصورة الأصلية، يرد ويندوز بأيقونة النوع. صار قصد يطلب الصورة المصغّرة الحقيقية أولاً، ولا يرجع إلى الأيقونة إلا إن لم توجد صورة.
2. **أمر `qasd stdio` كان يعلق ولا ينتهي**
   الأنابيب المسماة في ويندوز لا تستطيع إغلاق اتجاه واحد فقط، فكانت الخدمة تنتظر طلبات إلى الأبد. صار السطر الفارغ إشارة نهاية الطلبات، فتغلق الخدمة الاتصال بعد الرد.
3. **ترتيب كلمات شريط الحالة كان مقلوباً**
   امتزاج العربية بالإنجليزية في سطر واحد جعل "ms 0.9" و"GB" تظهر في غير مكانها. صارت الأجزاء اللاتينية معزولة باتجاهها الصحيح. واستُعملت علامات الاتجاه القديمة (LRE/PDF)، لأن علامات العزل الأحدث تظهر في ويندوز مربعاتٍ فارغة.
4. **نافذة البحث كانت تنهار عند رسم نص فارغ**
   دالة الرسم في ويندوز تقرأ مؤشر النص حتى لو كان فارغاً. صار النص الفارغ لا يُمرَّر إليها أصلاً.
5. **مسارات الأقراص الافتراضية (subst)**
   كان تغيير على قرص افتراضي يُسجَّل على القرص الحقيقي الذي يشير إليه. صار يُسجَّل على حرف القرص الذي حدث عليه التغيير.
6. **أمر الإزالة ومهمة المدير**
   إزالة التثبيت من حساب عادي لم تكن تستطيع إيقاف الخدمة التي تعمل بصلاحيات المدير. صارت تطلب الصلاحية بنافذة UAC واحدة ثم تكمل.

### English

**New:**

1. **The full right-click menu in the search window.** Right-click a result (or the menu key, or Shift+F10) for the file's real Explorer menu: open with, send to, cut, copy, create shortcut, delete, properties and your shell extensions (NanaZip, 7-Zip...). On top: open file location, copy path, and "what takes the space?" for folders. Shift+right-click shows the extended menu.
2. **USB drives and memory cards.** A drive plugged in is indexed and watched within about a quarter of a second; one unplugged leaves the results. Qasd lets go of a drive the moment Windows is asked to eject it, so "Safely Remove" never finds it in use. As administrator, an NTFS drive plugged in is read from its MFT too.
3. **Drag and drop.** Drag any result into another program (a mail, an editor, a chat, a folder): the file itself, copied, moved or linked, exactly as Explorer drags it.
4. **Preview pane.** The selected file's real thumbnail (images, videos, PDFs, folders), a text or code file's first lines in a monospaced font, or its icon; with its name, type, size and date. Made on its own thread, so typing never waits; cloud-only OneDrive files are never downloaded to preview them.
5. **Close and show buttons for the preview.** An ✕ in the pane's corner, and a preview button in the search box (lit while it shows). Ctrl+P too; the choice is remembered.
6. **Search history.** With an empty box, your last 30 searches: Enter runs one again, Delete forgets it. A search is kept when you act on a result (open, copy, drag), not on every keystroke.
7. **Size and date on every result, and whole folder sizes.** A folder's size is everything in it, at any depth, summed from the index with no disk access, and matches PowerShell's count.
8. **What takes the space** (a Filelight replacement). Ctrl+U, the pie button, or right-click a folder: its contents largest first with size, share and a bar; Enter goes in, Backspace up, Esc back to searching. All of C: (2 million entries) in about 8 ms from the index, so searching is not slowed at all.
9. **Developer and rights.** The EXEs' properties name **Ali Alhumedi** (company, copyright, product, version), and Qasd is listed in Windows' Installed apps (and uninstaller tools) with **Ali Alhumedi** as publisher, its version, icon, size and install date.
10. **Full uninstall from Windows Settings.** Uninstall in Installed apps removes everything: the program, the sign-in task, the start with Windows, the PATH entry, the index and settings, asking for administrator rights once if needed.
11. **`qasdw --usage [folder]`** opens the window straight on the disk usage view (handy for a desktop shortcut).

**Fixed:**

1. **The preview showed the file type's icon instead of the picture.** Asked for a thumbnail bigger than the image, the shell answers with the type icon; Qasd now asks for the thumbnail itself first and falls back to the icon only when there is none.
2. **`qasd stdio` never exited.** A named pipe can't close one direction, so the daemon waited for more requests forever; an empty line now ends the session and the daemon closes after answering.
3. **Mixed Arabic/English status lines came out of order** ("ms 0.9", "GB" in the wrong place). Latin runs are now kept left-to-right, with LRE/PDF marks (GDI draws the newer LRI/PDI isolates as boxes).
4. **The search window could crash drawing an empty string** (DrawTextW reads the pointer even for an empty string); empty strings are no longer passed to it.
5. **subst drives:** a change seen through a subst drive was recorded on the real drive it points to; it now stays on the drive letter it came from.
6. **Uninstall from a normal account couldn't stop the elevated daemon;** it now asks for administrator rights (one UAC prompt) and finishes.

---

## v0.3.1 — 2026-10-14

**بالعربية:** إصلاح استهلاك الذاكرة: بعد بناء الفهرس عبر MFT كانت الخدمة تحتفظ بنحو 650 ميغا بلا حاجة، لأن مدير الذاكرة لا يعيد الذاكرة إلا من الخيط الذي يطلب ذلك، بينما كانت خيوط التحليل المتوازية نائمة. صار التنظيف يجري على كل الخيوط، فتنزل الذاكرة من 983 ميغا إلى 10 ميغا.

**English:** After an MFT build the daemon kept ~650 MB committed: mimalloc returns pages only from the thread that collects, and the parser pool's threads were asleep. Every pool thread now collects too: 983 MB → 10 MB.

## v0.3.0 — 2026-10-14

**بالعربية:**
- **قراءة جدول ملفات NTFS (MFT) مباشرة** عند التشغيل بصلاحيات المدير: بناء الفهرس الكامل لمليوني ملف في 5.3 ثانية بدل 34 ثانية عند التشغيل الأول.
- **سجل تغييرات NTFS (USN Journal)** بدون صلاحيات مدير: بعد إعادة التشغيل يلحق قصد بالتغييرات في 0.08 ثانية بدل 3.4 ثوانٍ، ويستعيد ما يفوته إذا جاءت تغييرات كثيرة دفعة واحدة.
- **مشاكل حُلّت:** جدول MFT المجزأ (أماكنه المسجلة في سجلات إضافية عبر ATTRIBUTE_LIST)، والقراءة بلا ذاكرة مؤقتة وبعناوين محاذاة، ومنع ظهور نسخة الخدمة العادية قبل نسخة المدير أثناء التثبيت.

**English:**
- **Index straight from each NTFS drive's MFT** as administrator: a full index of 2 million entries in 5.3 s instead of 34 s on a cold first run.
- **Catch up from the NTFS change journal** (USN), no admin rights needed: after a restart, 0.08 s instead of 3.4 s; overflowed change notifications are replayed from it too.
- **Fixed:** fragmented $MFT (runs continued in extension records via $ATTRIBUTE_LIST); unbuffered, sector-aligned reads; install now starts the elevated daemon before the window.

## v0.2.0 — 2026-10-14

**بالعربية:** نافذة البحث بالاختصار **Alt+Q**: تظهر فوق ما تعمل عليه، والنتائج أثناء الكتابة مع أيقونات الملفات. Enter يفتح، وCtrl+Enter يفتح المجلد، وCtrl+C ينسخ المسار، وEsc يغلق. فيها أيقونة في شريط المهام، وتعمل مع تشغيل ويندوز، وتتبع المظهر الفاتح والداكن ودقة الشاشة. وفيها أيقونة البرنامج الجديدة.

**English:** The **Alt+Q** search window: over whatever you're doing, results as you type with file icons; Enter opens, Ctrl+Enter shows the folder, Ctrl+C copies the path, Esc closes. Tray icon, start with Windows, light/dark theme, per-monitor DPI, and the app's own icon.

## v0.1.0 — 2026-10-13

**بالعربية:** أول إصدار لويندوز. بحث في أسماء الملفات بتسامح مع الأخطاء الإملائية في كل الأقراص، وبحث مفهرس داخل الملفات، وتحديث لحظي، وفلاتر (ext: in: size: mtime: git: ...)، وخادم MCP لأدوات البرمجة. ومن المشاكل التي حُلّت: استُبدلت مقابس يونكس (AF_UNIX) بالأنابيب المسماة لأنها غير موثوقة في ويندوز، ونُقل رقم العملية إلى ملف مستقل لأن الملف المقفل لا يُقرأ في ويندوز، وتُحذف ملفات المقابس القديمة.

**English:** First Windows release: typo-tolerant name search on every drive, indexed search inside files, live updates, filters (ext: in: size: mtime: git: ...), an MCP server for coding agents. Fixed along the way: AF_UNIX sockets replaced by named pipes (unreliable on Windows); the pid moved to its own file (a locked file can't be read on Windows); stale socket files removed.
