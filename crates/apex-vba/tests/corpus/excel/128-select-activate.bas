' corpus-source: generated
' corpus-flagged: no
' corpus-prints: B2
Sub Main()
    Range("B2").Select
    Debug.Print ActiveCell.Address(False, False)
End Sub
