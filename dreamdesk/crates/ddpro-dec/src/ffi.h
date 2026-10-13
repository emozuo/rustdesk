// 生成接口绑定用的头文件包装（build.rs 里的 bindgen 读它）。
// hwcontext_vaapi.h 是 RustDesk 打过补丁的版本：带 libva 的运行时函数表 VAAPIDynLoadFunctions。
#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_vaapi.h>
#include <libavutil/pixdesc.h>
#include <va/va_vpp.h>
#include <va/va_drmcommon.h>
