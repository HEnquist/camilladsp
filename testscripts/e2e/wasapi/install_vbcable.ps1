# Installs the VB-Audio Virtual Cable driver on a headless Windows runner.
#
# Does what `devcon install <inf> <hwid>` does, through SetupAPI directly, so the suite
# needs neither devcon.exe nor a third party action that bundles it. The driver's signer
# is added to TrustedPublisher first, since the install is otherwise stopped by a prompt
# asking whether to trust the publisher, which nobody is there to answer.
#
# Usage: install_vbcable.ps1 -Dir <unpacked driver pack> [-Instances <n>]

param(
    [Parameter(Mandatory = $true)][string]$Dir,
    [int]$Instances = 1
)

$ErrorActionPreference = 'Stop'

$Inf = Join-Path (Resolve-Path $Dir) 'vbMmeCable64_win10.inf'
$HardwareId = 'VBAudioVACWDM'

# Trust every certificate the catalogs are signed with, rather than a .cer kept in the
# repo, so a new driver pack brings its own.
foreach ($cat in Get-ChildItem $Dir -Filter *.cat) {
    $sig = Get-AuthenticodeSignature $cat.FullName
    Write-Host "$($cat.Name): $($sig.Status), $($sig.SignerCertificate.Subject)"
    if ($sig.SignerCertificate) {
        $cer = Join-Path $env:RUNNER_TEMP "$($cat.BaseName).cer"
        Export-Certificate -Cert $sig.SignerCertificate -FilePath $cer | Out-Null
        certutil -f -addstore TrustedPublisher $cer | Out-Null
    }
}

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class DevInstall {
    const int DICD_GENERATE_ID = 1;
    const int SPDRP_HARDWAREID = 1;
    const int DIF_REGISTERDEVICE = 0x19;
    const int INSTALLFLAG_FORCE = 1;
    static readonly IntPtr INVALID_HANDLE_VALUE = new IntPtr(-1);

    [StructLayout(LayoutKind.Sequential)]
    struct SP_DEVINFO_DATA {
        public int cbSize;
        public Guid ClassGuid;
        public int DevInst;
        public IntPtr Reserved;
    }

    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool SetupDiGetINFClass(string infName, out Guid classGuid,
        StringBuilder className, int classNameSize, out int requiredSize);

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid classGuid, IntPtr hwndParent);

    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool SetupDiCreateDeviceInfo(IntPtr set, string deviceName, ref Guid classGuid,
        string description, IntPtr hwndParent, int flags, ref SP_DEVINFO_DATA data);

    [DllImport("setupapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool SetupDiSetDeviceRegistryProperty(IntPtr set, ref SP_DEVINFO_DATA data,
        int property, byte[] buffer, int size);

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiCallClassInstaller(int function, IntPtr set, ref SP_DEVINFO_DATA data);

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);

    [DllImport("newdev.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool UpdateDriverForPlugAndPlayDevices(IntPtr hwndParent, string hardwareId,
        string infPath, int flags, out bool rebootRequired);

    static void Check(bool ok, string what) {
        if (!ok) throw new Win32Exception(Marshal.GetLastWin32Error(), what);
    }

    // Creates a root enumerated device node with the hardware id, then points the driver
    // at every node with that id. Returns whether Windows asks for a reboot.
    public static bool Install(string inf, string hardwareId) {
        Guid classGuid;
        int required;
        StringBuilder className = new StringBuilder(32);
        Check(SetupDiGetINFClass(inf, out classGuid, className, className.Capacity, out required),
            "SetupDiGetINFClass");

        IntPtr set = SetupDiCreateDeviceInfoList(ref classGuid, IntPtr.Zero);
        if (set == INVALID_HANDLE_VALUE) Check(false, "SetupDiCreateDeviceInfoList");
        try {
            SP_DEVINFO_DATA data = new SP_DEVINFO_DATA();
            data.cbSize = Marshal.SizeOf(data);
            Check(SetupDiCreateDeviceInfo(set, className.ToString(), ref classGuid, null,
                IntPtr.Zero, DICD_GENERATE_ID, ref data), "SetupDiCreateDeviceInfo");
            // REG_MULTI_SZ, so the list ends with an extra null.
            byte[] id = Encoding.Unicode.GetBytes(hardwareId + "\0\0");
            Check(SetupDiSetDeviceRegistryProperty(set, ref data, SPDRP_HARDWAREID, id, id.Length),
                "SetupDiSetDeviceRegistryProperty");
            Check(SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, ref data),
                "SetupDiCallClassInstaller");
        } finally {
            SetupDiDestroyDeviceInfoList(set);
        }

        bool reboot;
        Check(UpdateDriverForPlugAndPlayDevices(IntPtr.Zero, hardwareId, inf, INSTALLFLAG_FORCE,
            out reboot), "UpdateDriverForPlugAndPlayDevices");
        return reboot;
    }
}
'@

for ($i = 1; $i -le $Instances; $i++) {
    $reboot = [DevInstall]::Install($Inf, $HardwareId)
    Write-Host "instance $i installed, reboot requested: $reboot"
}
