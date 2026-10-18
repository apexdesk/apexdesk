' corpus-source: generated
' corpus-prints: 15
' corpus-flagged: no
Attribute VB_Name = "Module1"
Option Explicit

Sub Main()
    Dim i As Long, total As Double
    For i = 1 To 5
        Cells(i, 1).Value = i
    Next i
    For i = 1 To 5
        total = total + Cells(i, 1).Value
    Next i
    Range("B1").Value = total
    Debug.Print Range("B1").Value
End Sub
