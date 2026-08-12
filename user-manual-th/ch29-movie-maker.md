# บทที่ 29 — Movie Maker (สร้างหนัง AI จากบทภาพยนตร์)

**Movie Maker** เปลี่ยนบทภาพยนตร์สั้น ๆ ที่คุณเขียน — ด้วยภาษาเล็ก ๆ ชื่อ
`.film` — ให้เป็นวิดีโอที่เสร็จสมบูรณ์: ตัวละครและฉากคงเส้นคงวา บทพูดภาษาไทย
(หรืออังกฤษ) เพลง และซับไตเติล คุณบรรยายช็อต ส่วนเครื่องมือรัน pipeline การ
generate ให้ และแสดงค่าใช้จ่ายให้ดูก่อนจ่ายจริง

บทนี้เป็นคู่มือเชิงงาน ส่วนรายละเอียดภายในเอนจิน (ไวยากรณ์ `.film` เต็ม,
compiler, backend) อยู่ใน technical manual ที่ `filmscript.md`

## 1. ติดตั้ง agent Movie Maker

Movie Maker มาในรูป **catalog agent** ไม่ใช่ built-in — tool ของมันจะซ่อน
จนกว่าคุณจะติดตั้ง (เพื่อให้ tool วิดีโอที่เสียเงินปิดไว้เป็น default):

```
/cloud get movie-maker-2
```

การติดตั้งลง folder จะได้ skill `.film` + GUI shell **Film Studio** (หน้าจอ
คลิก ๆ) คุณต้องมีสิทธิ์ gateway ของ thClaws.cloud (หรือ provider key) เพราะ
การ generate เสียเงิน

## 2. เตรียมภาพตัวละคร / ฉาก

backend รักษาให้ตัวละครหน้าตาเหมือนเดิมข้ามช็อตด้วยการยึด **ภาพอ้างอิง**
สร้างภาพตัวละครก่อน (TextToImage หรือใช้ภาพของคุณเอง — แต่บาง backend ปฏิเสธ
รูปใบหน้าคนจริงเป็น reference ใช้ภาพที่ generate สำหรับคน) แล้ว import ให้
`.film` อ้างได้:

- วางไว้ใน `.thclaws/film/assets/` (หรือใช้ `FilmAssetImport` สำหรับ base64)
- อ้างเป็น `@./assets/hero.png`

## 3. เขียนบท `.film`

ไฟล์ `.film` คือบทภาพยนตร์ที่มีโครงสร้างนิดหน่อย ตัวอย่างขั้นต่ำ:

```
film "Morning" {
  aspect: 16:9
  resolution: 720p
  char $mai = @./assets/mai.png voice:th-female-warm desc:"a young Thai woman"

  sequence "Kitchen" {
    scene: a sunny kitchen
    shot dialogue {
      $mai say "อรุณสวัสดิ์ค่ะ"
      camera: medium close-up
    }
  }
}
```

คุณเขียน **ตัวละคร/ฉาก/prop** (พร้อมภาพอ้างอิง + เสียง) จัดกลุ่ม **ช็อต** เป็น
**sequence** และในแต่ละช็อตเขียน action, บทพูด (`$ใคร say "…"`), และ directive
(`@duration: 6`, `@backend: veo`, `@continue_from: …`) โดย default backend
เป็นคนพูดบทเอง ส่วนเสียง `narrator` ทำ voice-over

## 4. ดูค่าใช้จ่ายก่อน แล้วค่อย generate

compile ก่อนเสมอ — ฟรีและทันที บอกได้ว่าหนังจะราคาเท่าไหร่:

- **`FilmCompile`** ตรวจบท + คืน **ประมาณการค่าใช้จ่าย** ต่อช็อต (USD)
- **`FilmGenerate`** render จริง **ต้องระบุ `budgetUsd`** — เป็นทั้งการยินยอม
  และเพดานตายตัว: การ generate จะหยุดก่อนช็อตที่จะทำให้ใช้จ่ายเกิน budget
  (ช็อตที่ render แล้วเก็บไว้ ปรับ budget ขึ้นแล้วรันด้วย `resume` เพื่อทำต่อ)

ใน Film Studio shell จะเป็นปุ่ม "ดูค่าใช้จ่าย → Generate" ส่วนในแชต agent
เรียก tool ให้เอง

หนังที่เสร็จ + ซับไตเติลจะอยู่ที่ `.thclaws/film/<job>/out/`
(`final.mp4`, `final.srt`)

## 5. รีวิวและ re-roll

ไม่พอใจช็อตไหน? ให้ agent **ดู** มัน (`WatchVideo` ดึง key frame + transcript
ให้ model เห็นผลจริง) แก้ช็อตนั้นใน `.film` แล้ว generate ใหม่ — จะ render ใหม่
**เฉพาะช็อตที่เปลี่ยน** (ที่เหลือใช้ cache) การวนแก้จึงถูก

## 6. เสียง

voice id (`th-female-warm`, `th-male-low`, `narrator`, …) map ไปยัง TTS provider
ใน `.thclaws/film/voices.json` ซึ่งแก้ได้เพื่อเลือก provider หรือเสียงอื่น เสียง
narrator ไทย default คือเสียง "Charon" ของ Gemini **model ไม่ใช่แค่ provider
เป็นตัวตัดสินคุณภาพภาษาไทย** — ค่า default เลือกมาให้พูดไทยได้ดี

## เคล็ดลับ

- เริ่มเล็ก ๆ (1 sequence, 2–3 ช็อตสั้น) แล้วดูค่าใช้จ่ายก่อนขยาย — วิดีโอคิด
  เงินตามวินาทีที่ output
- ใช้ภาพที่ generate (ไม่ใช่รูปถ่ายคนจริง) เป็น reference ตัวละคร
- อย่าต่อช็อตยาว: ช็อต `@continue_from` คิดเงินตามความยาวคลิปต้นทาง *บวก*
  ความยาวใหม่
