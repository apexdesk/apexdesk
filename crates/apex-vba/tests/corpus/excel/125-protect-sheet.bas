' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: False
Sub Main()
    ActiveSheet.Protect Password:="abc"
    Debug.Print ActiveSheet.ProtectContents
    ActiveSheet.Unprotect Password:="abc"
    Debug.Print ActiveSheet.ProtectContents
End Sub
