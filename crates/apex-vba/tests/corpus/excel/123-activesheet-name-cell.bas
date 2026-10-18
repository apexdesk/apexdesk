' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Sheet1
' corpus-prints: Sheet1!A1
Sub Main()
    Debug.Print ActiveSheet.Name
    Range("A1").Value = 1
    Debug.Print ActiveSheet.Name & "!" & Range("A1").Address(False, False)
End Sub
