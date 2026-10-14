' corpus-source: generated
' corpus-flagged: no
' corpus-prints: $C$5
' corpus-prints: C5
' corpus-prints: 5
' corpus-prints: 3
Sub Main()
    Debug.Print Cells(5, 3).Address
    Debug.Print Cells(5, 3).Address(False, False)
    Debug.Print Cells(5, 3).Row
    Debug.Print Cells(5, 3).Column
End Sub
