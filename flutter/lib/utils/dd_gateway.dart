// 受管版向网关取工作站列表、用域账号登录（主窗口首页 dd_home_page.dart 和 kiosk dd_kiosk_page.dart 共用）。
// 两页原来各写一份，已经分叉过一次（首页修了「回的不是 JSON 按服务器出错报」，kiosk 没跟上）；这里只管请求和解析，界面各管各的。
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/hbbs/hbbs.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/http_service.dart' as http;

/// 工作站现在能不能连：空闲 / 我自己在别处连着 / 别人在用 / 离线
enum DdWsState { available, mine, busy, offline }

/// 网关 /api/dd/workstations 列表里的一台
class DdWs {
  final String id;

  /// 显示的名字：网关给的工作站名，没给就是设备号（[named] 为假）
  final String name;

  /// 网关给了名字
  final bool named;
  final DdWsState state;

  /// 系统（网关原样给的，如「windows / Windows 11 Enterprise - 10.0.26200」；短名见 [ddOsShort]）
  final String os;

  /// 设备详情「设备信息」卡片的内容（网关的可选字段 [infoKeys]，没有的就不放）
  final Map<String, String> info;

  const DdWs(this.id, this.name, this.state, {this.named = true, this.os = '', this.info = const {}});

  /// 网关给的设备信息字段：主机名、当前登录的系统账号、IP、DreamDesk 版本、CPU、内存、我最近一次连上的时间
  static const infoKeys = ['hostname', 'os_username', 'ip', 'version', 'cpu', 'memory', 'last_connected'];

  /// 能连：空闲的、或我自己在别处连着的（再连一次接过来）
  bool get selectable => state == DdWsState.available || state == DdWsState.mine;
  bool get online => state != DdWsState.offline;

  /// 定时刷新时比较新旧列表，没变就不重画
  @override
  bool operator ==(Object other) =>
      other is DdWs &&
      id == other.id &&
      name == other.name &&
      named == other.named &&
      os == other.os &&
      state == other.state &&
      mapEquals(info, other.info);

  @override
  int get hashCode => Object.hash(id, name, os, state);
}

/// 系统的短名：「windows / Windows 11 Enterprise - 10.0.26200」→「Windows 11 Enterprise」
String ddOsShort(String os) => os.split(' / ').last.split(' - ').first.trim();

/// 系统类别（系统图标、背景按它选）。网关给的系统以类别开头（「windows / …」「linux / …」）；认不出的当 Windows
enum DdOsKind { windows, linux, mac }

DdOsKind ddOsKind(String os) {
  final p = os.toLowerCase();
  if (p.startsWith('linux')) return DdOsKind.linux;
  if (p.startsWith('mac')) return DdOsKind.mac;
  return DdOsKind.windows;
}

/// 取列表没成的原因：连不上服务器（网络问题） / 服务器回了错（含回的不是我们认得的格式）
enum DdFailure { network, server }

/// 取一次列表的结果：成功时 [items] 不为空；令牌被拒时 [rejected]；其余失败给 [failure] 和服务器回的说明 [message]（可能为空）
class DdWsFetch {
  final List<DdWs>? items;
  final bool rejected;
  final DdFailure? failure;
  final String message;
  const DdWsFetch._({this.items, this.rejected = false, this.failure, this.message = ''});
}

/// 取这个人能连的工作站。服务器回了话就不算「连不上」：状态码不对、回的不是 JSON（如反向代理的 502 网页）、
/// 字段类型不对，都按服务器出错报
Future<DdWsFetch> ddFetchWorkstations() async {
  final http.Response resp;
  try {
    final api = await bind.mainGetApiServer();
    resp = await http.get(Uri.parse('$api/api/dd/workstations'), headers: getHttpHeaders());
  } catch (e) {
    debugPrint('dd-gateway: listing workstations failed: $e');
    return const DdWsFetch._(failure: DdFailure.network);
  }
  if (resp.statusCode == 401) {
    debugPrint('dd-gateway: token rejected while listing workstations');
    return const DdWsFetch._(rejected: true);
  }
  final Object? body;
  try {
    body = jsonDecode(resp.body);
  } catch (e) {
    debugPrint('dd-gateway: workstation list is not JSON (HTTP ${resp.statusCode}): $e');
    return const DdWsFetch._(failure: DdFailure.server);
  }
  final error = body is Map && body['error'] is String ? body['error'] as String : '';
  final list = body is Map ? body['items'] : null;
  if (resp.statusCode != 200 || (list != null && list is! List)) {
    debugPrint('dd-gateway: workstation list failed (HTTP ${resp.statusCode})');
    return DdWsFetch._(failure: DdFailure.server, message: error);
  }
  String str(Object? v) => v is String ? v : '';
  final items = <DdWs>[];
  for (final m in (list as List?) ?? const []) {
    if (m is! Map) continue;
    final id = str(m['id']);
    if (id.isEmpty) continue;
    final name = str(m['name']);
    final state = m['online'] != true
        ? DdWsState.offline
        : (m['in_use_by_me'] == true ? DdWsState.mine : (m['in_use'] == true ? DdWsState.busy : DdWsState.available));
    items.add(DdWs(id, name.isEmpty ? id : name, state, named: name.isNotEmpty, os: str(m['os']), info: {
      for (final k in DdWs.infoKeys)
        if (str(m[k]).isNotEmpty) k: str(m[k]),
    }));
  }
  return DdWsFetch._(items: items);
}

/// 登录没成：[message] 是给用户看的（已翻译）
class DdSignInError {
  final String message;
  const DdSignInError(this.message);
}

/// 用域账号登录（网关 /api/login，由网关向 AD 验证；本机不认证、不存口令）。成功时存好令牌和用户信息、返回网关的应答；
/// 失败抛 [DdSignInError]。RustDesk 的 login() 会先改用户名（gFFI.userModel.userName），这里存好令牌之后才返回
Future<LoginResponse> ddSignIn(String user, String password) async {
  try {
    final resp = await gFFI.userModel.login(LoginRequest(
        username: user,
        password: password,
        id: await bind.mainGetMyId(),
        uuid: await bind.mainGetUuid(),
        autoLogin: true,
        type: HttpType.kAuthReqTypeAccount));
    final token = resp.access_token;
    // 服务端要二次验证（邮件 / 动态码）：AD 登录没有这一步，不做
    if (resp.type != HttpType.kAuthResTypeToken || token == null) throw DdSignInError(translate('Sign-in failed'));
    await bind.mainSetLocalOption(key: 'access_token', value: token);
    await bind.mainSetLocalOption(key: 'user_info', value: jsonEncode(resp.user ?? {}));
    // 登录状态以令牌为准（首页按用户名变化去对令牌）：login() 改用户名时令牌还没存，存好后再通知一次
    gFFI.userModel.userName.refresh();
    return resp;
  } on DdSignInError {
    rethrow;
  } on RequestException catch (e) {
    debugPrint('dd-gateway: sign-in failed: ${e.statusCode}');
    throw DdSignInError(e.cause.isNotEmpty ? translate(e.cause) : translate('Sign-in failed'));
  } on FormatException catch (e) {
    // 服务器回了话、但不是 JSON（如反向代理的错误网页）：RustDesk 的 login() 解析失败时原样抛出。不是「连不上」
    debugPrint('dd-gateway: sign-in reply is not JSON: $e');
    throw DdSignInError(translate('Sign-in failed'));
  } on TypeError catch (e) {
    // 回的 JSON 字段类型不对（RustDesk 解析 LoginResponse 时抛的）
    debugPrint('dd-gateway: sign-in reply is malformed: $e');
    throw DdSignInError(translate('Sign-in failed'));
  } catch (e) {
    debugPrint('dd-gateway: sign-in failed: $e');
    throw DdSignInError(translate('Cannot reach the server'));
  }
}
