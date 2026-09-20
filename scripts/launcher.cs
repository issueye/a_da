using System;
using System.Diagnostics;
using System.IO;

class Launcher {
    [STAThread]
    static int Main(string[] args) {
        try {
            string baseDir = AppDomain.CurrentDomain.BaseDirectory;
            string coreExe = Path.Combine(baseDir, "a-da-core.exe");
            if (!File.Exists(coreExe)) {
                coreExe = Path.Combine(baseDir, "a-da-engine.exe");
            }
            if (!File.Exists(coreExe)) {
                return 1;
            }

            ProcessStartInfo psi = new ProcessStartInfo();
            psi.FileName = coreExe;
            psi.Arguments = string.Join(" ", args);
            // 继承调用方的当前工作目录，确保读取工作区配置正常
            psi.WorkingDirectory = Environment.CurrentDirectory;
            // 核心：CREATE_NO_WINDOW 确保系统天然不为核心进程分配控制台黑框
            psi.CreateNoWindow = true;
            psi.UseShellExecute = false;

            Process p = Process.Start(psi);
            return p != null ? 0 : 2;
        } catch {
            return 3;
        }
    }
}
