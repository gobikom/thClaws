# บทที่ 30 — Job Artifacts

*ตั้งแต่ v0.88.0*

สั่งงาน thClaws worker ผ่าน HTTP แล้วเก็บ**ไฟล์**ที่มันผลิต — ประกาศไว้
ต่อ job, ถูก freeze พร้อม hash ทันทีที่ run จบ, ดาวน์โหลดด้วย Bearer
token ใบเดียวกับที่ใช้สั่งงาน และส่งไฟล์ตั้งต้นเข้าไปได้ด้วยวิธีเดียวกัน
ทั้งหมดนี้ทำให้ instance `--serve` ไหนก็ได้กลายเป็น worker node ที่ต่อ
เป็น pipeline ได้:

```
coder (เครื่อง A) ──artifacts──▶ orchestrator ของคุณ ──inputs──▶ reviewer (เครื่อง B)
```

ถ้าเคยใช้ artifacts ของ GitHub Actions หรือ GitLab CI — นี่คือแนวคิด
เดียวกัน (output ที่ประกาศไว้ของ job หนึ่ง เรียกคืนได้ด้วย id) แต่ใช้กับ
agent run โดย "job" คือ `session_id` ที่ `/agent/run` ตอบกลับมาอยู่แล้ว

## ทำไมไม่ใช้ workspace sync?

Sync surface (`/workspace/sync/*`, บทที่ 27) mirror *ทั้ง workspace*
และออกแบบมาสำหรับเครือข่ายที่เชื่อถือได้ (มี tunnel หรือ ForwardAuth
คั่นหน้า) orchestrator ภายนอกที่ถือแค่ API token เจอช่องว่างสามข้อ:
ไม่มี auth ที่รองรับอย่างเป็นทางการ, ไม่รู้ว่า job ไหนสร้างไฟล์อะไร,
และมี race — รายการไฟล์กับตัวไฟล์เป็นคนละ request ไฟล์อาจถูกแก้
ระหว่างนั้น Job Artifacts ปิดครบทั้งสามข้อ: Bearer auth, scope ต่อ job,
และ manifest ที่ hash ถูก fix ตอนเก็บ

## ทำไมไม่ใช้ A2A (หรือ ACP)?

คำถามที่เจอบ่อย: มีโปรโตคอลกลางสำหรับ agent คุยกันอยู่แล้ว ทำไมต้อง
มี endpoint เฉพาะของ thClaws?

**A2A (Agent2Agent Protocol)** — โปรโตคอลเปิดจาก Google (ปัจจุบันอยู่
ใต้ Linux Foundation) สำหรับให้ agent ต่างค่ายสั่งงานกันได้: ประกาศ
ความสามารถผ่าน Agent Card, สั่งเป็น task ผ่าน JSON-RPC, stream
ความคืบหน้าด้วย SSE ส่วน **ACP** มีสองตัวที่ชื่อชนกัน — *Agent
Communication Protocol* (IBM/BeeAI) ซึ่งควบรวมเข้ากับ A2A ไปแล้ว
และ *Agent Client Protocol* (Zed) ซึ่งเป็นโปรโตคอล editor ↔ coding
agent สำหรับฝัง agent ในหน้าจอ editor — คนละเรื่องกับการส่งงานข้าม
เครื่อง

เหตุผลที่ Job Artifacts ยังต้องมี: **A2A เป็นชั้นสนทนา ส่วน artifacts
เป็นชั้น storage** — artifact ของ A2A คือ message parts ที่ไหลกลับมา
กับ task ระหว่างคุย ไม่ได้ให้สัญญาเรื่องความคงทน แต่ artifact ของ
thClaws คือไฟล์จริงใน workspace ที่ถูก freeze เป็น snapshot + sha256
ตอน run จบ แล้ว**ดึงซ้ำได้ทีหลังด้วย id** ต่อให้ไฟล์ต้นทางถูกแก้ไปแล้ว
(semantics แบบ CI artifacts ตามที่เทียบไว้ข้างบน) และในทางปฏิบัติ
orchestrator ที่ถือแค่ API token ใช้ curl สามคำสั่งก็ครบ ไม่ต้องมี
A2A client ถ้าวันหน้า thClaws มี A2A facade มันก็จะเสิร์ฟผลลัพธ์จาก
artifact store ตัวนี้อยู่ดี — สองอย่างนี้ไม่ได้แทนกัน

## เริ่มใช้งาน

เปิด worker:

```bash
THCLAWS_API_TOKEN=secret thclaws --serve --port 8443
```

**1. (ถ้าต้องการ) ส่งไฟล์ตั้งต้น** ก่อนสั่งงาน:

```bash
curl -X POST http://worker:8443/v1/inputs \
  -H "Authorization: Bearer secret" -H "Content-Type: application/json" \
  -d '{"files":[{"path":"inputs/brief.txt","content_base64":"'"$(base64 < brief.txt)"'"}]}'
```

ไฟล์จะลงใต้ `inputs/` ใน workspace ของ worker (ดู
[กติกาการวางไฟล์](#input-rules))

**2. สั่งงานพร้อม `collect_files`** — glob บอกว่าอะไรคือ output ของ job นี้:

```bash
curl -X POST http://worker:8443/agent/run \
  -H "Authorization: Bearer secret" -H "Content-Type: application/json" \
  -d '{"prompt":"อ่าน inputs/brief.txt แล้วเขียนรายงานลง reports/summary.md",
       "collect_files":["reports/*.md"]}'
# → { "session_id": "sess-abc…", "summary": "...", ... }
```

เมื่อ run จบ ไฟล์ที่ match จะถูก **copy เข้า snapshot ของ session** และ
คำนวณ sha256 — ตั้งแต่วินาทีนั้น artifact จะไม่เปลี่ยนอีก ต่อให้ไฟล์จริง
ใน workspace ถูกแก้ทีหลัง สิ่งที่ดาวน์โหลดได้ก็ยังเป็นชุดเดิม

**3. ดึง manifest แล้วดึงไฟล์:**

```bash
curl -H "Authorization: Bearer secret" \
  http://worker:8443/v1/sessions/sess-abc…/artifacts
# → { "artifacts": [ { "id":"a1", "path":"reports/summary.md",
#                      "size":180, "sha256":"67aeef…" } ], ... }

curl -H "Authorization: Bearer secret" -o summary.md \
  http://worker:8443/v1/sessions/sess-abc…/artifacts/a1
# response มี header `x-sha256` — เทียบกับ manifest ได้ทันที
```

## สรุป endpoint

ทั้งหมดใช้ Bearer auth เดียวกับ `/v1` (`THCLAWS_API_TOKEN`)

| Endpoint | หน้าที่ |
|---|---|
| `POST /agent/run` + `"collect_files": ["glob", …]` | ประกาศ output ของ run; snapshot + hash ตอนจบ (ครบทั้ง sync / streaming / async) |
| `GET /v1/sessions/{sid}/artifacts` | manifest ที่นิ่งแล้ว: `id`, `path`, `size`, `sha256` ต่อไฟล์ + `skipped` ถ้าเกิน cap |
| `GET /v1/sessions/{sid}/artifacts/{aid}` | ไฟล์หนึ่งไฟล์ เสิร์ฟจาก **snapshot** (ไม่ใช่ไฟล์สด) — `aid` ใช้ id (`a1`, …) หรือ path ตรงๆ |
| `POST /v1/inputs` | วางไฟล์เข้า workspace ก่อนสั่งงาน — body: `{"workspace_dir"?, "files":[{"path","content_base64"}]}` |

ทั้ง GET และ `/v1/inputs` รับ `workspace_dir` เสริม (query parameter
สำหรับ GET) ตรวจสอบแบบเดียวกับ `/agent/run`; ไม่ใส่ = ใช้ working
directory ของ daemon

## กติกาการวางไฟล์ (inputs) {#input-rules}

`POST /v1/inputs` เข้มโดยตั้งใจ:

- path ต้องเป็น **relative**, ห้ามมี `..`, ห้ามแตะ `.thclaws/` กับ `.git/`
- ค่าเริ่มต้นวางได้เฉพาะใต้ **`inputs/`** — ขยายด้วย env ฝั่ง worker:
  `THCLAWS_INPUTS_PREFIXES="inputs/,data/"` หรือ
  `THCLAWS_INPUTS_PREFIXES="*"` (ทุกที่ใน workspace ยกเว้น `.thclaws/`
  กับ `.git/` ที่ห้ามเสมอ)
- limit: ≤ 100 ไฟล์/request, decode แล้ว ≤ 64 MB · response ตอบ
  `sha256` ของทุกไฟล์ที่เขียนเพื่อให้ฝั่งส่งตรวจได้

## ขีดจำกัดการเก็บ artifacts

snapshot ต่อ run สูงสุด **256 ไฟล์ / 300 MB** — ไฟล์ที่ match แต่เกิน
cap จะไปอยู่ในรายการ `skipped` ของ manifest (เห็นชัดว่าถูกตัด ไม่หาย
เงียบๆ) การเก็บข้าม `.thclaws/`, `.git/`, `node_modules/` เสมอ

## เปิด Bearer ให้ workspace sync (Tier 1)

ถ้าต้องการ mirror ทั้ง workspace (บทที่ 27) จาก orchestrator ที่ถือแค่
API token ให้ opt-in ฝั่ง worker:

```bash
THCLAWS_SYNC_REQUIRE_AUTH=1 THCLAWS_API_TOKEN=secret thclaws --serve
```

ทุก request ไป `/workspace/sync/*` จะต้องมี
`Authorization: Bearer <token>` — ไม่ต้องมี tunnel หรือ ForwardAuth
ไม่ตั้ง flag = พฤติกรรมเดิมทุกประการ deployment เก่าไม่กระทบ

## ที่เก็บบน disk

snapshot อยู่ใน workspace ของ worker ที่
`.thclaws/state/artifacts/<session_id>/` (`manifest.json` + `files/`)
— อยู่ใต้ `.thclaws/state/` จึงถูก gitignore และไม่ติดไปกับการ pack/
publish agent · ไม่มีการลบอัตโนมัติ ถ้า worker รันยาวควรลบ directory
ของ session เก่าเป็นระยะ

## ดูเพิ่มเติม

- [บทที่ 3](ch03-working-directory-and-modes.md) — โหมด `--serve` และ
  `THCLAWS_API_TOKEN`
- [บทที่ 19](ch19-scheduling.md#heartbeats) — heartbeat schedule ใช้คู่กับ
  artifacts สำหรับ loop ผลิต-แล้ว-เก็บต่อเนื่อง
- [บทที่ 27](ch27-thclaws-cloud.md) — workspace sync ทางเลือกแบบทั้ง workspace
- Technical manual: `job-artifacts.md` สำหรับรายละเอียดระดับ wire
