// 受管版端口映射窗口的内容（用户 10-27 定方案 A：只换外观，不动布局和行为）。原型：dreamdesk/design/prototype/PortMap.html。
// port_forward_page.dart 里只留一行钩子；读写映射、刷新列表仍走它那边传进来的回调，这里只管画。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';

import 'dd_icons.dart';
import 'dd_restyle.dart';
import 'dd_theme.dart';

const _kColWidth = 140.0;
const _kActionWidth = 120.0;
const _kGap = 12.0;
const _kRowPadding = 20.0;

/// 表格的最小宽：三个固定列 + 操作列 + 四个间隔 + 左右内边距 + 给主机列留的 120。窗口比它窄时表格整块横着滚，不溢出
const _kMinTableWidth = 3 * _kColWidth + _kActionWidth + 4 * _kGap + 2 * _kRowPadding + 120;

final _portFormatter = FilteringTextInputFormatter.allow(RegExp(
    r'^([0-9]|[1-9]\d|[1-9]\d{2}|[1-9]\d{3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5])$'));

/// [rows]：现有的映射（本机端口、远程主机、远程端口）。[onAdd] / [onRemove] / [onNewRdp] 做完事、刷新列表由调用方负责
class DdPortForwardView extends StatefulWidget {
  final bool isRDP;
  final List<(int, String, int)> rows;
  final Future<void> Function(int localPort, String remoteHost, int remotePort) onAdd;
  final Future<void> Function(int localPort) onRemove;
  final VoidCallback onNewRdp;
  const DdPortForwardView({
    super.key,
    required this.isRDP,
    required this.rows,
    required this.onAdd,
    required this.onRemove,
    required this.onNewRdp,
  });

  @override
  State<DdPortForwardView> createState() => _DdPortForwardViewState();
}

class _DdPortForwardViewState extends State<DdPortForwardView> {
  final _local = TextEditingController();
  final _host = TextEditingController();
  final _remote = TextEditingController();

  @override
  void dispose() {
    _local.dispose();
    _host.dispose();
    _remote.dispose();
    super.dispose();
  }

  Future<void> _add() async {
    final localPort = int.tryParse(_local.text);
    final remotePort = int.tryParse(_remote.text);
    if (localPort == null || remotePort == null) return;
    // 主机填了但只有空格：照原来的判断拒绝添加（留空才当 localhost）
    if (_host.text.isNotEmpty && _host.text.trim().isEmpty) return;
    final host = _host.text.trim();
    await widget.onAdd(localPort, host.isEmpty ? 'localhost' : host, remotePort);
    // 等添加的请求期间窗口关了，控制器已经销毁
    if (!mounted) return;
    _local.clear();
    _host.clear();
    _remote.clear();
  }

  /// 一行五列：本机端口 | 远程主机（占剩下的）| 远程端口 | 状态 | 操作
  Widget _row(double height, List<Widget> cells, {bool last = false}) => Container(
        height: height,
        padding: const EdgeInsets.symmetric(horizontal: _kRowPadding),
        decoration: BoxDecoration(border: last ? null : const Border(bottom: BorderSide(color: DdColors.paneLine))),
        child: Row(children: [
          SizedBox(width: _kColWidth, child: cells[0]),
          const SizedBox(width: _kGap),
          Expanded(child: cells[1]),
          const SizedBox(width: _kGap),
          SizedBox(width: _kColWidth, child: cells[2]),
          const SizedBox(width: _kGap),
          SizedBox(width: _kColWidth, child: cells[3]),
          const SizedBox(width: _kGap),
          SizedBox(width: _kActionWidth, child: Align(alignment: Alignment.centerLeft, child: cells[4])),
        ]),
      );

  Widget _head(String text) => Text(translate(text), style: const TextStyle(fontSize: 12, color: DdColors.ink2));

  Widget _cell(String text) => Text(text, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 13));

  Widget _table(List<(int, String, int)> rows) {
    const none = SizedBox.shrink();
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 4),
      decoration: BoxDecoration(color: DdColors.surface, borderRadius: BorderRadius.circular(DdRadius.lg)),
      child: Column(children: [
        _row(40, [_head('Local Port'), _head('Remote Host'), _head('Remote Port'), widget.isRDP ? none : _head('Status'), none]),
        if (widget.isRDP)
          _row(52, [
            ElevatedButton(onPressed: widget.onNewRdp, style: ddCompactButton, child: Text(translate('New RDP'))),
            _cell('localhost'),
            _cell('RDP'),
            none,
            none,
          ], last: true)
        else ...[
          _row(52, [
            DdInput(controller: _local, inputFormatters: [_portFormatter], label: translate('Local Port')),
            DdInput(controller: _host, hint: 'localhost', label: translate('Remote Host')),
            DdInput(controller: _remote, inputFormatters: [_portFormatter], label: translate('Remote Port')),
            none,
            ElevatedButton(onPressed: _add, style: ddCompactButton, child: Text(translate('Add'))),
          ], last: rows.isEmpty),
          for (var i = 0; i < rows.length; i++)
            _row(48, [
              _cell(rows[i].$1.toString()),
              _cell(rows[i].$2),
              _cell(rows[i].$3.toString()),
              Row(children: [
                Container(width: 6, height: 6, decoration: const BoxDecoration(color: DdColors.online, shape: BoxShape.circle)),
                const SizedBox(width: 6),
                Text(translate('Listening'), style: const TextStyle(fontSize: 13, color: DdColors.ink2)),
              ]),
              TextButton(
                onPressed: () => widget.onRemove(rows[i].$1),
                style: ddCompactButton.merge(TextButton.styleFrom(foregroundColor: DdColors.dangerText)),
                child: Text(translate('Delete')),
              ),
            ], last: i == rows.length - 1),
        ],
      ]),
    );
  }

  @override
  Widget build(BuildContext context) {
    final rows = widget.rows;
    return SingleChildScrollView(
      padding: const EdgeInsets.symmetric(horizontal: 32, vertical: 24),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        // 原来的绿条（「正在等待隧道连接」+「不要关闭本窗口」）：有映射或 RDP 时才出现，照旧
        if (rows.isNotEmpty || widget.isRDP) ...[
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            decoration: BoxDecoration(color: DdColors.warnBg, borderRadius: BorderRadius.circular(DdRadius.md)),
            child: Row(children: [
              ddIcon(DdIcon.circleAlert, color: DdColors.busy),
              const SizedBox(width: 8),
              Expanded(child: Text(translate('not_close_tcp_tip'), style: const TextStyle(fontSize: 13, height: 20 / 13))),
            ]),
          ),
          const SizedBox(height: 16),
        ],
        LayoutBuilder(
          builder: (context, box) => box.maxWidth >= _kMinTableWidth
              ? _table(rows)
              : SingleChildScrollView(
                  scrollDirection: Axis.horizontal, child: SizedBox(width: _kMinTableWidth, child: _table(rows))),
        ),
      ]),
    );
  }
}
